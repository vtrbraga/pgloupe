//! Aiven for PostgreSQL, through the `avn` CLI.
//!
//! **Experimental:** the exact output and paging of `avn service logs` are not documented in
//! detail; this reads the JSON the client prints (`avn --json service logs`), accepting either a
//! list of entries or an object with a `logs` list, and fetches the most recent entries each run.

use std::path::PathBuf;

use jiff::Timestamp;
use serde_json::Value;

use super::{
    FetchError, LogProvider, LogSink, ProviderState, Request, Summary,
    cmd::Tool,
    entries::{self, Cursor, Entry},
};

const INSTALL_HINT: &str =
    "install the Aiven client (`pip install aiven-client`) and run `avn user login`";
/// Entries asked for per run.
const LIMIT: usize = 5_000;

/// An Aiven service.
#[derive(Debug, Clone)]
pub struct Aiven {
    tool: Tool,
    project: String,
    service: String,
}

fn explain(stderr: &str) -> &'static str {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("authentication")
        || lower.contains("not logged in")
        || lower.contains("unauthorized")
    {
        "not signed in: run `avn user login`, or set AIVEN_AUTH_TOKEN"
    } else if lower.contains("not found") {
        "no such service: check --instance (the service name) and --project"
    } else {
        ""
    }
}

impl Aiven {
    /// A provider for `service` in `project`. `program` is the `avn` executable.
    #[must_use]
    pub fn new(program: PathBuf, project: String, service: String) -> Self {
        Aiven {
            tool: Tool {
                program,
                timeout_secs: 300,
                install_hint: INSTALL_HINT,
                env: Vec::new(),
            },
            project,
            service,
        }
    }
}

fn parse_entries(bytes: &[u8]) -> Result<Vec<Entry>, FetchError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|e| FetchError::Output {
        provider: "aiven",
        what: "service logs",
        detail: e.to_string(),
    })?;
    let list = value
        .get("logs")
        .and_then(Value::as_array)
        .or_else(|| value.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for item in list {
        let ts = item.get("time").and_then(Value::as_str).and_then(|t| {
            let t = if t.contains('T') {
                t.to_owned()
            } else {
                t.replacen(' ', "T", 1)
            };
            let t = if t.ends_with('Z') || t[10..].contains('+') || t[10..].contains('-') {
                t
            } else {
                format!("{t}Z")
            };
            t.parse::<Timestamp>().ok()
        });
        let text = ["msg", "message"]
            .iter()
            .find_map(|k| item.get(*k).and_then(Value::as_str));
        if let (Some(ts), Some(text)) = (ts, text) {
            out.push(Entry {
                ts,
                id: format!("{:x}", xxhash_rust::xxh3::xxh3_64(text.as_bytes())),
                text: text.to_owned(),
            });
        }
    }
    Ok(out)
}

impl LogProvider for Aiven {
    fn name(&self) -> &'static str {
        "aiven"
    }

    fn check(&self) -> Result<(), FetchError> {
        self.tool
            .run_ok(&["--version".to_owned()], "--version", |_| "")?;
        self.tool
            .run_ok(
                &["user".to_owned(), "info".to_owned()],
                "user info",
                explain,
            )
            .map(|_| ())
    }

    fn fetch(
        &self,
        request: &Request,
        sink: &mut dyn LogSink,
        state: &mut ProviderState,
    ) -> Result<Summary, FetchError> {
        let args: Vec<String> = [
            "--json",
            "service",
            "logs",
            &self.service,
            "--project",
            &self.project,
            "--limit",
            &LIMIT.to_string(),
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        let out = self.tool.run_ok(&args, "service logs", explain)?;
        let mut batch = parse_entries(&out)?;
        batch.retain(|e| e.ts >= request.since);
        let mut cursor = Cursor::load(state, "cursor");
        let (files, bytes) = entries::write(
            sink,
            &format!("aiven-{}", self.service),
            &batch,
            &mut cursor,
        )?;
        cursor.store(state, "cursor");
        sink.checkpoint(state)?;
        Ok(Summary {
            files,
            bytes,
            restarted: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_documented_shapes_are_read() {
        let object = br#"{"logs":[{"time":"2026-10-14T10:00:01.123456Z","unit":"postgres","msg":"LOG:  hello"}],"offset":"x"}"#;
        let list = br#"[{"time":"2026-10-14 10:00:02","msg":"LOG:  again"}]"#;
        assert_eq!(parse_entries(object).unwrap()[0].text, "LOG:  hello");
        assert_eq!(
            parse_entries(list).unwrap()[0].ts.to_string(),
            "2026-10-14T10:00:02Z"
        );
        assert_eq!(parse_entries(b"{}").unwrap().len(), 0);
        assert!(matches!(
            parse_entries(b"plain text"),
            Err(FetchError::Output { .. })
        ));
    }

    #[test]
    fn failures_get_the_fix() {
        assert!(explain("ERROR: authentication failed").contains("avn user login"));
        assert!(explain("service not found").contains("--project"));
    }
}
