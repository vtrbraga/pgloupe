//! Azure Database for PostgreSQL Flexible Server, through the `az` CLI.
//!
//! **Experimental:** written from the Azure documentation (`az postgres flexible-server
//! server-logs list` and `download`; the server parameter `logfiles.download_enable` must be on),
//! not yet run against a real account.

use std::{
    fs,
    path::{Path, PathBuf},
};

use jiff::Timestamp;
use serde_json::Value;

use super::{
    FetchError, LogProvider, LogSink, ProviderState, Request, Summary, aws::safe_name, cmd::Tool,
};

const INSTALL_HINT: &str =
    "install the Azure CLI (https://learn.microsoft.com/cli/azure/) and run `az login`";

/// A Flexible Server's log files.
#[derive(Debug, Clone)]
pub struct AzureFlexible {
    tool: Tool,
    resource_group: String,
    server: String,
    subscription: Option<String>,
}

fn explain(stderr: &str) -> &'static str {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("az login")
        || lower.contains("please run 'az login'")
        || lower.contains("not logged in")
    {
        "not signed in: run `az login`"
    } else if lower.contains("resourcenotfound") || lower.contains("could not be found") {
        "no such server: check --instance (the server name), --resource-group and --subscription"
    } else if lower.contains("authorizationfailed") || lower.contains("does not have authorization")
    {
        "the account needs Reader on the server (Microsoft.DBforPostgreSQL/flexibleServers/logFiles/read)"
    } else if lower.contains("download_enable") || lower.contains("logfiles") {
        "enable the server parameter logfiles.download_enable (and set logfiles.retention_days)"
    } else {
        ""
    }
}

impl AzureFlexible {
    /// A provider for `server` in `resource_group`. `program` is the `az` executable.
    #[must_use]
    pub fn new(
        program: PathBuf,
        resource_group: String,
        server: String,
        subscription: Option<String>,
    ) -> Self {
        AzureFlexible {
            tool: Tool {
                program,
                timeout_secs: 300,
                install_hint: INSTALL_HINT,
                env: vec![("AZURE_CORE_ONLY_SHOW_ERRORS", "true")],
            },
            resource_group,
            server,
            subscription,
        }
    }

    fn args(&self, operation: &str) -> Vec<String> {
        let mut args: Vec<String> = ["postgres", "flexible-server", "server-logs", operation]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        args.extend([
            "--resource-group".to_owned(),
            self.resource_group.clone(),
            "--server-name".to_owned(),
            self.server.clone(),
        ]);
        if let Some(subscription) = &self.subscription {
            args.extend(["--subscription".to_owned(), subscription.clone()]);
        }
        args
    }
}

/// `(remote name, last modified)` of each file in an `az ... server-logs list` answer. Field
/// names are read tolerantly: the CLI has used `camelCase` and `snake_case` spellings.
fn parse_list(bytes: &[u8]) -> Result<Vec<(String, Timestamp)>, FetchError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|e| FetchError::Output {
        provider: "azure-flexible",
        what: "server-logs list",
        detail: e.to_string(),
    })?;
    let items = value.as_array().cloned().unwrap_or_default();
    let mut files = Vec::new();
    for item in items {
        let name = item.get("name").and_then(Value::as_str);
        let modified = ["lastModifiedTime", "last_modified_time", "lastModified"]
            .iter()
            .find_map(|k| item.get(*k).and_then(Value::as_str));
        if let (Some(name), Some(modified)) = (name, modified) {
            let text = if modified.ends_with('Z') || modified.contains('+') {
                modified.to_owned()
            } else {
                format!("{modified}Z")
            };
            if let Ok(ts) = text.parse::<Timestamp>() {
                files.push((name.to_owned(), ts));
            }
        }
    }
    files.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    Ok(files)
}

/// The first regular file under `dir` (the CLI saves downloads below its working directory).
fn find_download(dir: &Path) -> Option<PathBuf> {
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_download(&path) {
                return Some(found);
            }
        } else {
            return Some(path);
        }
    }
    None
}

impl LogProvider for AzureFlexible {
    fn name(&self) -> &'static str {
        "azure-flexible"
    }

    fn check(&self) -> Result<(), FetchError> {
        self.tool
            .run_ok(&["--version".to_owned()], "--version", |_| "")?;
        self.tool
            .run_ok(
                &[
                    "account".to_owned(),
                    "show".to_owned(),
                    "--output".to_owned(),
                    "none".to_owned(),
                ],
                "account show",
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
        let mut list_args = self.args("list");
        list_args.extend(["--output".to_owned(), "json".to_owned()]);
        let listing = self.tool.run_ok(
            &list_args,
            "postgres flexible-server server-logs list",
            explain,
        )?;
        let mut summary = Summary::default();
        for (remote, modified) in parse_list(&listing)? {
            if modified < request.since {
                continue;
            }
            let local = safe_name(&remote);
            let tmp = std::env::temp_dir().join(format!(
                "pgloupe-az-{}-{}",
                std::process::id(),
                Timestamp::now().as_nanosecond()
            ));
            fs::create_dir_all(&tmp).map_err(|source| FetchError::Io {
                path: tmp.display().to_string(),
                source,
            })?;
            let mut args = self.args("download");
            args.extend(["--name".to_owned(), remote.clone()]);
            let result = self.tool.run_in(&args, Some(&tmp)).and_then(|out| {
                if out.code == Some(0) {
                    Ok(())
                } else {
                    let hint = explain(&out.stderr);
                    Err(FetchError::Command {
                        program: self.tool.program.display().to_string(),
                        command: "postgres flexible-server server-logs download".to_owned(),
                        status: out
                            .code
                            .map_or_else(|| "killed".to_owned(), |c| format!("exit {c}")),
                        stderr: out.stderr,
                        hint: if hint.is_empty() {
                            String::new()
                        } else {
                            format!(" — {hint}")
                        },
                    })
                }
            });
            let downloaded = result.and_then(|()| {
                let path = find_download(&tmp).ok_or(FetchError::Output {
                    provider: "azure-flexible",
                    what: "server-logs download",
                    detail: format!("the CLI reported success but saved no file for {remote}"),
                })?;
                fs::read(&path).map_err(|source| FetchError::Io {
                    path: path.display().to_string(),
                    source,
                })
            });
            drop(fs::remove_dir_all(&tmp));
            let content = downloaded?;
            // Downloads are whole files; the file only grows, so add what is new.
            let stored = usize::try_from(sink.stored(&local)).unwrap_or(0);
            let tail = if content.len() < stored {
                sink.reset(&local)?;
                summary.restarted.push(local.clone());
                &*content
            } else {
                &content[stored..]
            };
            if !tail.is_empty() {
                sink.append(&local, tail)?;
                summary.bytes += tail.len() as u64;
                summary.files.push(local);
            }
            state.insert(format!("seen:{remote}"), modified.to_string());
            sink.checkpoint(state)?;
        }
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_listing_is_read_with_either_field_spelling() {
        let camel = br#"[{"lastModifiedTime":"2026-10-14T10:00:00+00:00","name":"serverlogs/postgresql-2026-10-14_100000.log","sizeInKb":12,"type":"x","url":"u"}]"#;
        let snake = br#"[{"last_modified_time":"2026-10-14T11:00:00","name":"serverlogs/b.log"}]"#;
        let a = parse_list(camel).unwrap();
        assert_eq!(a[0].0, "serverlogs/postgresql-2026-10-14_100000.log");
        let b = parse_list(snake).unwrap();
        assert_eq!(b[0].1.to_string(), "2026-10-14T11:00:00Z");
        assert_eq!(parse_list(b"[]").unwrap().len(), 0);
        assert!(matches!(
            parse_list(b"not json"),
            Err(FetchError::Output { .. })
        ));
    }

    #[test]
    fn failures_get_the_fix() {
        assert!(explain("ERROR: Please run 'az login' to setup account.").contains("az login"));
        assert!(
            explain("(ResourceNotFound) The Resource ... could not be found")
                .contains("--resource-group")
        );
        assert!(explain("logfiles.download_enable is off").contains("logfiles.download_enable"));
    }
}
