//! Heroku Postgres, through the `heroku` CLI.
//!
//! **Experimental:** written from the CLI's documented behaviour (`heroku logs --ps postgres`); it
//! sees only the most recent lines Heroku keeps (about 1,500), so it is for spot checks, not
//! history. Nothing here has been run against a real app.

use std::path::PathBuf;

use jiff::Timestamp;

use super::{
    FetchError, LogProvider, LogSink, ProviderState, Request, Summary,
    cmd::Tool,
    entries::{self, Cursor, Entry},
};

const INSTALL_HINT: &str = "install the Heroku CLI (https://devcenter.heroku.com/articles/heroku-cli) and run `heroku login`";
/// Lines asked for (Heroku keeps about this many).
const LINES: &str = "1500";

/// A Heroku app with a Postgres add-on.
#[derive(Debug, Clone)]
pub struct Heroku {
    tool: Tool,
    app: String,
}

fn explain(stderr: &str) -> &'static str {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("heroku login")
        || lower.contains("not logged in")
        || lower.contains("invalid credentials")
    {
        "not signed in: run `heroku login`"
    } else if lower.contains("couldn't find that app") || lower.contains("not found") {
        "no such app: check --instance (the app name)"
    } else {
        ""
    }
}

impl Heroku {
    /// A provider for `app`. `program` is the `heroku` executable.
    #[must_use]
    pub fn new(program: PathBuf, app: String) -> Self {
        Heroku {
            tool: Tool {
                program,
                timeout_secs: 120,
                install_hint: INSTALL_HINT,
                env: vec![("HEROKU_ANALYTICS", "0")],
            },
            app,
        }
    }
}

/// Log lines as `heroku logs` prints them: `2026-10-14T10:00:01.123456+00:00 app[postgres.7]: text`.
/// Lines that do not start with a timestamp continue the previous entry.
fn parse_lines(text: &str) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    for line in text.lines() {
        let head: Option<(Timestamp, &str)> = line
            .split_once(' ')
            .and_then(|(stamp, rest)| stamp.parse::<Timestamp>().ok().map(|ts| (ts, rest)));
        match head {
            Some((ts, rest)) => {
                // Drop the source (`app[postgres.7]:`), keep the message.
                let message = rest.split_once("]: ").map_or(rest, |(_, m)| m);
                out.push(Entry {
                    ts,
                    id: format!("{:x}", xxhash_rust::xxh3::xxh3_64(line.as_bytes())),
                    text: message.to_owned(),
                });
            }
            None => {
                if let Some(last) = out.last_mut() {
                    last.text.push('\n');
                    last.text.push_str(line);
                }
            }
        }
    }
    out
}

impl LogProvider for Heroku {
    fn name(&self) -> &'static str {
        "heroku"
    }

    fn check(&self) -> Result<(), FetchError> {
        self.tool
            .run_ok(&["--version".to_owned()], "--version", |_| "")?;
        self.tool
            .run_ok(&["auth:whoami".to_owned()], "auth:whoami", explain)
            .map(|_| ())
    }

    fn fetch(
        &self,
        request: &Request,
        sink: &mut dyn LogSink,
        state: &mut ProviderState,
    ) -> Result<Summary, FetchError> {
        let args: Vec<String> = ["logs", "--ps", "postgres", "--app", &self.app, "-n", LINES]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let out = self.tool.run_ok(&args, "logs", explain)?;
        let mut batch = parse_lines(&String::from_utf8_lossy(&out));
        batch.retain(|e| e.ts >= request.since);
        let mut cursor = Cursor::load(state, "cursor");
        let (files, bytes) =
            entries::write(sink, &format!("heroku-{}", self.app), &batch, &mut cursor)?;
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
    fn lines_are_split_into_entries_with_continuations() {
        let text = "2026-10-14T10:00:01.123456+00:00 app[postgres.7]: [DATABASE] [1-1] LOG:  duration: 5 ms\n\
                    second line of the same entry\n\
                    2026-10-14T10:00:02+00:00 app[postgres.7]: [DATABASE] [2-1] LOG:  next\n";
        let entries = parse_lines(text);
        assert_eq!(entries.len(), 2);
        assert!(
            entries[0]
                .text
                .starts_with("[DATABASE] [1-1] LOG:  duration")
        );
        assert!(entries[0].text.ends_with("second line of the same entry"));
        assert_ne!(entries[0].id, entries[1].id);
    }

    #[test]
    fn failures_get_the_fix() {
        assert!(
            explain(" ›   Error: Invalid credentials provided. Run heroku login")
                .contains("heroku login")
        );
        assert!(explain("Couldn't find that app.").contains("--instance"));
    }
}
