//! RDS and Aurora PostgreSQL logs that are exported to `CloudWatch` Logs, through the `aws` CLI.
//!
//! **Experimental:** written from the AWS documentation (`aws logs filter-log-events`, log group
//! `/aws/rds/instance/<id>/postgresql`), not yet run against a real account.

use jiff::Timestamp;
use serde::Deserialize;

use super::{
    FetchError, LogProvider, LogSink, ProviderState, Request, Summary,
    aws::AwsCli,
    entries::{self, Cursor, Entry},
};

/// Pages read in one run (a guard against a token that never ends).
const MAX_PAGES: u32 = 100_000;

/// A `CloudWatch` log group with Postgres logs in it.
#[derive(Debug, Clone)]
pub struct AwsCloudWatch {
    cli: AwsCli,
    group: String,
}

#[derive(Debug, Deserialize)]
struct Page {
    #[serde(default)]
    events: Vec<Event>,
    #[serde(default, rename = "nextToken")]
    next_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Event {
    #[serde(default, rename = "eventId")]
    id: String,
    timestamp: i64,
    message: String,
}

impl AwsCloudWatch {
    /// A provider for log group `group`.
    #[must_use]
    pub fn new(cli: AwsCli, group: String) -> Self {
        AwsCloudWatch { cli, group }
    }

    /// The log group RDS exports an instance's PostgreSQL log to.
    #[must_use]
    pub fn instance_group(instance: &str) -> String {
        format!("/aws/rds/instance/{instance}/postgresql")
    }
}

impl LogProvider for AwsCloudWatch {
    fn name(&self) -> &'static str {
        "aws-cloudwatch"
    }

    fn check(&self) -> Result<(), FetchError> {
        self.cli.check()
    }

    fn fetch(
        &self,
        request: &Request,
        sink: &mut dyn LogSink,
        state: &mut ProviderState,
    ) -> Result<Summary, FetchError> {
        let mut cursor = Cursor::load(state, "cursor");
        let start = cursor.ts.map_or(request.since, |ts| ts.max(request.since));
        let mut summary = Summary::default();
        let mut token: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut args = self.cli.base("logs", "filter-log-events");
            args.extend([
                "--log-group-name".to_owned(),
                self.group.clone(),
                "--start-time".to_owned(),
                start.as_millisecond().to_string(),
                "--no-paginate".to_owned(),
            ]);
            if let Some(until) = request.until {
                args.extend(["--end-time".to_owned(), until.as_millisecond().to_string()]);
            }
            if let Some(token) = &token {
                args.extend(["--next-token".to_owned(), token.clone()]);
            }
            let out = self.cli.call(&args, "logs filter-log-events")?;
            let page: Page = serde_json::from_slice(&out).map_err(|e| FetchError::Output {
                provider: "aws-cloudwatch",
                what: "filter-log-events",
                detail: e.to_string(),
            })?;
            let batch: Vec<Entry> = page
                .events
                .into_iter()
                .filter_map(|event| {
                    Some(Entry {
                        ts: Timestamp::from_millisecond(event.timestamp).ok()?,
                        id: event.id,
                        text: event.message,
                    })
                })
                .collect();
            let (files, bytes) = entries::write(sink, &self.group, &batch, &mut cursor)?;
            summary.bytes += bytes;
            for file in files {
                if !summary.files.contains(&file) {
                    summary.files.push(file);
                }
            }
            cursor.store(state, "cursor");
            sink.checkpoint(state)?;
            match page.next_token {
                Some(next) if token.as_deref() != Some(next.as_str()) => token = Some(next),
                _ => break,
            }
        }
        Ok(summary)
    }
}
