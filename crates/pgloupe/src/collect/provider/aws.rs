//! Amazon RDS and Aurora PostgreSQL, through the `aws` CLI.
//!
//! Supported: this is the connector tested against a real account.

use std::{path::PathBuf, thread, time::Duration};

use jiff::Timestamp;
use serde::Deserialize;

use super::{
    FetchError, LogProvider, LogSink, ProviderState, Request, Summary,
    cmd::{Output, Tool},
};

/// Calls retried when AWS says to slow down.
const RETRIES: u32 = 4;
/// Portions fetched for one file before giving up (a guard against a marker that never ends).
const MAX_PORTIONS: u32 = 1_000_000;

const INSTALL_HINT: &str =
    "install the AWS CLI (https://docs.aws.amazon.com/cli/) or pass its path with --aws-bin";

/// Where RDS/Aurora logs are read from.
#[derive(Debug, Clone)]
pub struct AwsRds {
    tool: Tool,
    instance: String,
    region: Option<String>,
    profile: Option<String>,
    retry_delay: Duration,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LogFiles {
    #[serde(default, rename = "DescribeDBLogFiles")]
    describe_db_log_files: Vec<LogFile>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LogFile {
    #[serde(rename = "LogFileName")]
    name: String,
    #[serde(default)]
    last_written: i64,
    #[serde(default)]
    size: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Portion {
    #[serde(default)]
    log_file_data: Option<String>,
    #[serde(default)]
    marker: Option<String>,
    #[serde(default)]
    additional_data_pending: bool,
}

/// What to try for the failures AWS reports most.
fn explain(stderr: &str) -> &'static str {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("unable to locate credentials") || lower.contains("no credentials") {
        "no AWS credentials found: run `aws configure`, `aws sso login`, or pass --profile"
    } else if lower.contains("expiredtoken")
        || lower.contains("token has expired")
        || lower.contains("sso session")
    {
        "the AWS session expired: sign in again (`aws sso login` or refresh the credentials)"
    } else if lower.contains("accessdenied") || lower.contains("not authorized") {
        "the role needs rds:DescribeDBLogFiles and rds:DownloadDBLogFilePortion on the instance"
    } else if lower.contains("dbinstancenotfound") {
        "no such instance: check --instance and --region"
    } else if lower.contains("could not connect to the endpoint")
        || lower.contains("could not resolve")
    {
        "cannot reach AWS: check the network and --region"
    } else {
        ""
    }
}

fn is_throttle(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("throttl")
        || lower.contains("rate exceeded")
        || lower.contains("too many requests")
}

/// A file name that is safe as a local path component.
#[must_use]
pub fn safe_name(remote: &str) -> String {
    let flat: String = remote
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | '\0') {
                '_'
            } else {
                c
            }
        })
        .collect();
    if flat.is_empty() || flat.chars().all(|c| c == '.') {
        format!("_{flat}")
    } else {
        flat
    }
}

impl AwsRds {
    /// A provider for `instance`. `program` is the `aws` executable.
    #[must_use]
    pub fn new(
        program: PathBuf,
        instance: String,
        region: Option<String>,
        profile: Option<String>,
    ) -> Self {
        AwsRds {
            tool: Tool {
                program,
                timeout_secs: 120,
                install_hint: INSTALL_HINT,
                env: vec![("AWS_PAGER", "")],
            },
            instance,
            region,
            profile,
            retry_delay: Duration::from_secs(1),
        }
    }

    /// Waits this long (doubling) before retrying a throttled call.
    #[must_use]
    pub fn with_retry_delay(mut self, delay: Duration) -> Self {
        self.retry_delay = delay;
        self
    }

    fn base(&self, service: &str, operation: &str) -> Vec<String> {
        let mut args = vec![service.to_owned(), operation.to_owned()];
        if let Some(region) = &self.region {
            args.push("--region".to_owned());
            args.push(region.clone());
        }
        if let Some(profile) = &self.profile {
            args.push("--profile".to_owned());
            args.push(profile.clone());
        }
        args.push("--output".to_owned());
        args.push("json".to_owned());
        args
    }

    /// Runs one call, retrying while AWS throttles.
    fn call(&self, args: &[String], command: &str) -> Result<Vec<u8>, FetchError> {
        let mut delay = self.retry_delay;
        for attempt in 0..=RETRIES {
            let output: Output = self.tool.run(args)?;
            if output.code == Some(0) {
                return Ok(output.stdout);
            }
            if attempt < RETRIES && is_throttle(&output.stderr) {
                thread::sleep(delay);
                delay *= 2;
                continue;
            }
            let hint = explain(&output.stderr);
            return Err(FetchError::Command {
                program: self.tool.program.display().to_string(),
                command: command.to_owned(),
                status: output
                    .code
                    .map_or_else(|| "killed".to_owned(), |c| format!("exit {c}")),
                stderr: output.stderr,
                hint: if hint.is_empty() {
                    String::new()
                } else {
                    format!(" — {hint}")
                },
            });
        }
        unreachable_attempts(command)
    }

    fn list(&self, since: Timestamp) -> Result<Vec<LogFile>, FetchError> {
        let mut args = self.base("rds", "describe-db-log-files");
        args.extend([
            "--db-instance-identifier".to_owned(),
            self.instance.clone(),
            "--file-last-written".to_owned(),
            since.as_millisecond().to_string(),
        ]);
        let out = self.call(&args, "rds describe-db-log-files")?;
        let mut files = serde_json::from_slice::<LogFiles>(&out)
            .map_err(|e| FetchError::Output {
                provider: "aws-rds",
                what: "describe-db-log-files",
                detail: e.to_string(),
            })?
            .describe_db_log_files;
        files.sort_by(|a, b| {
            a.last_written
                .cmp(&b.last_written)
                .then(a.name.cmp(&b.name))
        });
        Ok(files)
    }

    fn portion(&self, file: &str, marker: &str) -> Result<Portion, FetchError> {
        let mut args = self.base("rds", "download-db-log-file-portion");
        args.extend([
            "--db-instance-identifier".to_owned(),
            self.instance.clone(),
            "--log-file-name".to_owned(),
            file.to_owned(),
            "--marker".to_owned(),
            marker.to_owned(),
            "--no-paginate".to_owned(),
        ]);
        let out = self.call(&args, "rds download-db-log-file-portion")?;
        serde_json::from_slice(&out).map_err(|e| FetchError::Output {
            provider: "aws-rds",
            what: "download-db-log-file-portion",
            detail: e.to_string(),
        })
    }
}

fn unreachable_attempts(command: &str) -> Result<Vec<u8>, FetchError> {
    Err(FetchError::Output {
        provider: "aws-rds",
        what: "retry loop",
        detail: format!("{command} gave no result"),
    })
}

impl LogProvider for AwsRds {
    fn name(&self) -> &'static str {
        "aws-rds"
    }

    fn experimental(&self) -> bool {
        false
    }

    fn check(&self) -> Result<(), FetchError> {
        self.tool
            .run_ok(&["--version".to_owned()], "--version", |_| "")?;
        let mut args = self.base("sts", "get-caller-identity");
        args.retain(|a| a != "json" && a != "--output");
        self.call(&args, "sts get-caller-identity").map(|_| ())
    }

    fn fetch(
        &self,
        request: &Request,
        sink: &mut dyn LogSink,
        state: &mut ProviderState,
    ) -> Result<Summary, FetchError> {
        let mut summary = Summary::default();
        for file in self.list(request.since)? {
            let local = safe_name(&file.name);
            let key = format!("marker:{}", file.name);
            let size = u64::try_from(file.size).unwrap_or(0);
            let mut stored = sink.stored(&local);
            if stored > size {
                // The instance's copy is smaller than ours: it was rotated or recreated.
                sink.reset(&local)?;
                state.remove(&key);
                summary.restarted.push(local.clone());
                stored = 0;
            }
            if stored == size && state.contains_key(&key) {
                continue;
            }
            let mut marker = state.get(&key).cloned().unwrap_or_else(|| "0".to_owned());
            let mut wrote = false;
            for _ in 0..MAX_PORTIONS {
                let portion = self.portion(&file.name, &marker)?;
                let data = portion.log_file_data.unwrap_or_default();
                if !data.is_empty() {
                    sink.append(&local, data.as_bytes())?;
                    summary.bytes += data.len() as u64;
                    wrote = true;
                }
                let next = portion.marker.unwrap_or_else(|| marker.clone());
                if portion.additional_data_pending && next == marker && data.is_empty() {
                    return Err(FetchError::Output {
                        provider: "aws-rds",
                        what: "download-db-log-file-portion",
                        detail: format!("the marker {marker:?} does not advance for {}", file.name),
                    });
                }
                marker = next;
                state.insert(key.clone(), marker.clone());
                if !portion.additional_data_pending {
                    break;
                }
            }
            if wrote && !summary.files.contains(&local) {
                summary.files.push(local);
            }
            sink.checkpoint(state)?;
        }
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_names_become_safe_local_ones() {
        assert_eq!(
            safe_name("error/postgresql.log.2026-10-14-10"),
            "error_postgresql.log.2026-10-14-10"
        );
        assert_eq!(safe_name("../../etc/passwd"), ".._.._etc_passwd");
        assert_eq!(safe_name(".."), "_..");
        assert_eq!(safe_name(""), "_");
        assert!(!safe_name("a/b\\c").contains(['/', '\\']));
    }

    #[test]
    fn failures_get_the_fix_that_matches() {
        assert!(explain("Unable to locate credentials. You can configure credentials by running \"aws configure\".").contains("aws configure"));
        assert!(explain("An error occurred (ExpiredToken) when calling").contains("sign in"));
        assert!(explain("An error occurred (AccessDenied)").contains("rds:DescribeDBLogFiles"));
        assert!(explain("An error occurred (DBInstanceNotFound)").contains("--instance"));
        assert_eq!(explain("something else"), "");
        assert!(is_throttle(
            "An error occurred (Throttling) when calling the X operation: Rate exceeded"
        ));
    }
}
