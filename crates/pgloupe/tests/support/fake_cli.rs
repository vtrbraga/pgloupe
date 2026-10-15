//! A stand-in for a vendor CLI: a shell script that replays canned replies by argument match.

#![allow(dead_code, reason = "each test file uses a part")]
#![allow(
    clippy::expect_used,
    reason = "test scaffolding: a failed write should stop the test"
)]
#![allow(
    clippy::format_push_string,
    reason = "readability of a generated script"
)]

use std::{collections::BTreeMap, fs, os::unix::fs::PermissionsExt as _, path::PathBuf};

use pgloupe::collect::provider::{FetchError, LogSink, ProviderState};

/// One canned reply.
pub enum Reply {
    /// Exit 0 with this on standard output.
    Out(String),
    /// Exit with `code`, printing `stderr`.
    Fail { code: i32, stderr: String },
}

struct Rule {
    patterns: Vec<String>,
    replies: Vec<Reply>,
}

/// Builds the script.
pub struct FakeCli {
    dir: PathBuf,
    rules: Vec<Rule>,
}

impl FakeCli {
    /// A fake in a fresh temporary directory.
    pub fn new(label: &str) -> FakeCli {
        let dir = std::env::temp_dir().join(format!(
            "pgloupe-fake-{}-{label}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        drop(fs::remove_dir_all(&dir));
        fs::create_dir_all(&dir).expect("temp dir");
        FakeCli {
            dir,
            rules: Vec::new(),
        }
    }

    /// Replies to calls whose arguments contain every pattern; successive calls get successive
    /// replies and the last one repeats.
    pub fn on(mut self, patterns: &[&str], replies: Vec<Reply>) -> FakeCli {
        self.rules.push(Rule {
            patterns: patterns.iter().map(|p| (*p).to_owned()).collect(),
            replies,
        });
        self
    }

    /// Writes the script and returns its path.
    pub fn program(&self) -> PathBuf {
        let mut script = String::from("#!/bin/sh\n");
        script.push_str(&format!(
            "echo \"$*\" >> '{}/calls.log'\nargs=\"$*\"\n",
            self.dir.display()
        ));
        for (i, rule) in self.rules.iter().enumerate() {
            for (k, reply) in rule.replies.iter().enumerate() {
                let (out, err, code) = match reply {
                    Reply::Out(text) => (text.as_str(), "", 0),
                    Reply::Fail { code, stderr } => ("", stderr.as_str(), *code),
                };
                fs::write(self.dir.join(format!("r{i}.{k}.out")), out).expect("write");
                fs::write(self.dir.join(format!("r{i}.{k}.err")), err).expect("write");
                fs::write(self.dir.join(format!("r{i}.{k}.code")), code.to_string())
                    .expect("write");
            }
            let cond = rule
                .patterns
                .iter()
                .map(|p| {
                    format!(
                        "printf '%s' \"$args\" | grep -qF -- '{}'",
                        p.replace('\'', "'\\''")
                    )
                })
                .collect::<Vec<_>>()
                .join(" && ");
            let dir = self.dir.display();
            let last = rule.replies.len() - 1;
            script.push_str(&format!(
                "if {cond}; then\n  n=$(cat '{dir}/c{i}' 2>/dev/null || echo 0); echo $((n+1)) > '{dir}/c{i}'\n  [ $n -gt {last} ] && n={last}\n  cat \"{dir}/r{i}.$n.out\"; cat \"{dir}/r{i}.$n.err\" >&2; exit $(cat \"{dir}/r{i}.$n.code\")\nfi\n"
            ));
        }
        script.push_str("echo \"unexpected call: $args\" >&2\nexit 99\n");
        let path = self.dir.join("fake");
        fs::write(&path, script).expect("write script");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    }

    /// The argument lines the fake was called with, in order.
    pub fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.dir.join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

/// A sink that keeps everything in memory.
#[derive(Default)]
pub struct MemorySink {
    pub files: BTreeMap<String, Vec<u8>>,
}

impl LogSink for MemorySink {
    fn stored(&self, name: &str) -> u64 {
        self.files.get(name).map_or(0, |b| b.len() as u64)
    }

    fn reset(&mut self, name: &str) -> Result<(), FetchError> {
        self.files.remove(name);
        Ok(())
    }

    fn append(&mut self, name: &str, bytes: &[u8]) -> Result<(), FetchError> {
        self.files
            .entry(name.to_owned())
            .or_default()
            .extend_from_slice(bytes);
        Ok(())
    }

    fn checkpoint(&mut self, _state: &ProviderState) -> Result<(), FetchError> {
        Ok(())
    }
}
