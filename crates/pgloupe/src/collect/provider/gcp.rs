//! Cloud SQL for PostgreSQL, through the `gcloud` CLI and Cloud Logging.
//!
//! **Experimental:** written from the Google documentation (`gcloud logging read` on the log
//! `cloudsql.googleapis.com/postgres.log`); the layout of each entry's text (`textPayload` versus
//! `jsonPayload.message`) is read tolerantly and has not been checked against a real instance.

use std::path::PathBuf;

use jiff::Timestamp;
use serde_json::Value;

use super::{
    FetchError, LogProvider, LogSink, ProviderState, Request, Summary,
    cmd::Tool,
    entries::{self, Cursor, Entry},
};

const INSTALL_HINT: &str =
    "install the Google Cloud CLI (https://cloud.google.com/sdk) and run `gcloud auth login`";
/// Entries asked for per call.
const PAGE: usize = 1_000;
/// Calls in one run.
const MAX_PAGES: u32 = 10_000;

/// A Cloud SQL instance's PostgreSQL log.
#[derive(Debug, Clone)]
pub struct GcpCloudSql {
    tool: Tool,
    project: String,
    instance: String,
}

fn explain(stderr: &str) -> &'static str {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("gcloud auth login")
        || lower.contains("no credentialed accounts")
        || lower.contains("reauthentication")
    {
        "not signed in: run `gcloud auth login`"
    } else if lower.contains("permission_denied") || lower.contains("does not have permission") {
        "the account needs roles/logging.viewer on the project"
    } else if lower.contains("not found") && lower.contains("project") {
        "no such project: check --project"
    } else {
        ""
    }
}

impl GcpCloudSql {
    /// A provider for `instance` in `project`. `program` is the `gcloud` executable.
    #[must_use]
    pub fn new(program: PathBuf, project: String, instance: String) -> Self {
        GcpCloudSql {
            tool: Tool {
                program,
                timeout_secs: 300,
                install_hint: INSTALL_HINT,
                env: Vec::new(),
            },
            project,
            instance,
        }
    }

    /// The Cloud Logging filter for entries at or after `from`.
    #[must_use]
    pub fn filter(&self, from: Timestamp, until: Option<Timestamp>) -> String {
        let end = until.map_or_else(String::new, |until| format!(" AND timestamp<\"{until}\""));
        format!(
            "resource.type=\"cloudsql_database\" AND resource.labels.database_id=\"{}:{}\" AND logName=\"projects/{}/logs/cloudsql.googleapis.com%2Fpostgres.log\" AND timestamp>=\"{from}\"{end}",
            self.project, self.instance, self.project
        )
    }
}

/// Entries of a `gcloud logging read --format=json` answer.
fn parse_entries(bytes: &[u8]) -> Result<Vec<Entry>, FetchError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|e| FetchError::Output {
        provider: "gcp-cloudsql",
        what: "logging read",
        detail: e.to_string(),
    })?;
    let mut out = Vec::new();
    for item in value.as_array().cloned().unwrap_or_default() {
        let Some(ts) = item
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|t| t.parse::<Timestamp>().ok())
        else {
            continue;
        };
        let text = item
            .get("textPayload")
            .and_then(Value::as_str)
            .or_else(|| item.pointer("/jsonPayload/message").and_then(Value::as_str));
        let Some(text) = text else {
            continue;
        };
        out.push(Entry {
            ts,
            id: item
                .get("insertId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            text: text.to_owned(),
        });
    }
    Ok(out)
}

impl LogProvider for GcpCloudSql {
    fn name(&self) -> &'static str {
        "gcp-cloudsql"
    }

    fn check(&self) -> Result<(), FetchError> {
        self.tool
            .run_ok(&["--version".to_owned()], "--version", |_| "")?;
        let args: Vec<String> = [
            "auth",
            "list",
            "--filter=status:ACTIVE",
            "--format=value(account)",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        let out = self.tool.run_ok(&args, "auth list", explain)?;
        if String::from_utf8_lossy(&out).trim().is_empty() {
            return Err(FetchError::Command {
                program: self.tool.program.display().to_string(),
                command: "auth list".to_owned(),
                status: "no active account".to_owned(),
                stderr: String::new(),
                hint: " — not signed in: run `gcloud auth login`".to_owned(),
            });
        }
        Ok(())
    }

    fn fetch(
        &self,
        request: &Request,
        sink: &mut dyn LogSink,
        state: &mut ProviderState,
    ) -> Result<Summary, FetchError> {
        let mut cursor = Cursor::load(state, "cursor");
        let mut from = cursor.ts.map_or(request.since, |ts| ts.max(request.since));
        let prefix = format!("cloudsql-{}", self.instance);
        let mut summary = Summary::default();
        for _ in 0..MAX_PAGES {
            let args = vec![
                "logging".to_owned(),
                "read".to_owned(),
                self.filter(from, request.until),
                "--project".to_owned(),
                self.project.clone(),
                "--format".to_owned(),
                "json".to_owned(),
                "--order".to_owned(),
                "asc".to_owned(),
                "--limit".to_owned(),
                PAGE.to_string(),
            ];
            let out = self.tool.run_ok(&args, "logging read", explain)?;
            let batch = parse_entries(&out)?;
            let (files, bytes) = entries::write(sink, &prefix, &batch, &mut cursor)?;
            summary.bytes += bytes;
            for file in files {
                if !summary.files.contains(&file) {
                    summary.files.push(file);
                }
            }
            cursor.store(state, "cursor");
            sink.checkpoint(state)?;
            // A full page may have more behind it: continue from its newest entry.
            let newest = batch.iter().map(|e| e.ts).max();
            match newest {
                Some(ts) if batch.len() >= PAGE && ts > from => from = ts,
                _ => break,
            }
        }
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filter_names_the_instance_the_log_and_the_start() {
        let provider = GcpCloudSql::new("gcloud".into(), "my-proj".to_owned(), "prod".to_owned());
        let filter = provider.filter("2026-10-14T10:00:00Z".parse().unwrap(), None);
        assert!(filter.contains("database_id=\"my-proj:prod\""), "{filter}");
        assert!(filter.contains("cloudsql.googleapis.com%2Fpostgres.log"));
        assert!(filter.contains("timestamp>=\"2026-10-14T10:00:00Z\""));
    }

    #[test]
    fn entries_use_the_text_payload_or_a_json_message() {
        let json = br#"[
          {"timestamp":"2026-10-14T10:00:01.5Z","insertId":"a","textPayload":"2026-10-14 10:00:01 UTC [1]: LOG:  hi"},
          {"timestamp":"2026-10-14T10:00:02Z","insertId":"b","jsonPayload":{"message":"from json"}},
          {"timestamp":"2026-10-14T10:00:03Z","insertId":"c"},
          {"insertId":"d","textPayload":"no timestamp"}
        ]"#;
        let entries = parse_entries(json).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].text, "from json");
    }

    #[test]
    fn failures_get_the_fix() {
        assert!(explain("ERROR: (gcloud.logging.read) You do not currently have an active account selected. Please run: gcloud auth login").contains("gcloud auth login"));
        assert!(
            explain("PERMISSION_DENIED: The caller does not have permission")
                .contains("logging.viewer")
        );
    }
}
