//! Running a vendor's command-line tool: argument lists only, a time limit, bounded output.

use std::{
    io::Read as _,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use super::FetchError;

/// How much of a tool's stderr is kept in an error.
const STDERR_KEPT: usize = 600;

/// What a finished tool printed.
#[derive(Debug, Clone)]
pub struct Output {
    /// Exit code (None if killed by a signal).
    pub code: Option<i32>,
    /// Standard output.
    pub stdout: Vec<u8>,
    /// Standard error.
    pub stderr: String,
}

/// A tool to run, with its time limit and the hint to show when it is missing.
#[derive(Debug, Clone)]
pub struct Tool {
    /// Program name or path.
    pub program: PathBuf,
    /// Seconds before it is killed.
    pub timeout_secs: u64,
    /// What to say when it is not installed.
    pub install_hint: &'static str,
    /// Environment variables to set for the tool (for example to switch off a pager).
    pub env: Vec<(&'static str, &'static str)>,
}

impl Tool {
    /// Runs the tool with `args` (never through a shell).
    ///
    /// # Errors
    ///
    /// Fails if the tool is missing, cannot start, or runs past its limit.
    pub fn run(&self, args: &[String]) -> Result<Output, FetchError> {
        let shown = self.program.display().to_string();
        let mut child = Command::new(&self.program)
            .args(args)
            .envs(self.env.iter().copied())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    FetchError::MissingCli {
                        program: shown.clone(),
                        hint: self.install_hint,
                    }
                } else {
                    FetchError::Command {
                        program: shown.clone(),
                        command: String::new(),
                        status: "could not start".to_owned(),
                        stderr: error.to_string(),
                        hint: String::new(),
                    }
                }
            })?;
        let mut out = child.stdout.take();
        let mut err = child.stderr.take();
        let stdout_reader = thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(out) = out.as_mut() {
                drop(out.read_to_end(&mut buf));
            }
            buf
        });
        let stderr_reader = thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(err) = err.as_mut() {
                drop(err.read_to_end(&mut buf));
            }
            buf
        });
        let deadline = Instant::now() + Duration::from_secs(self.timeout_secs);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() >= deadline => {
                    drop(child.kill());
                    drop(child.wait());
                    return Err(FetchError::Timeout {
                        program: shown,
                        seconds: self.timeout_secs,
                    });
                }
                Ok(None) => thread::sleep(Duration::from_millis(20)),
                Err(error) => {
                    return Err(FetchError::Command {
                        program: shown,
                        command: String::new(),
                        status: "could not be waited for".to_owned(),
                        stderr: error.to_string(),
                        hint: String::new(),
                    });
                }
            }
        };
        let stdout = stdout_reader.join().unwrap_or_default();
        let stderr_bytes = stderr_reader.join().unwrap_or_default();
        let mut stderr = String::from_utf8_lossy(&stderr_bytes).trim().to_owned();
        if stderr.len() > STDERR_KEPT {
            let mut end = STDERR_KEPT;
            while !stderr.is_char_boundary(end) {
                end -= 1;
            }
            stderr.truncate(end);
            stderr.push('…');
        }
        Ok(Output {
            code: status.code(),
            stdout,
            stderr,
        })
    }

    /// Runs the tool and requires success, turning a failure into an error that carries `hint`
    /// when `explain` recognises the stderr.
    ///
    /// # Errors
    ///
    /// Fails like [`Tool::run`], and when the tool exits non-zero.
    pub fn run_ok(
        &self,
        args: &[String],
        command: &str,
        explain: fn(&str) -> &'static str,
    ) -> Result<Vec<u8>, FetchError> {
        let output = self.run(args)?;
        if output.code == Some(0) {
            return Ok(output.stdout);
        }
        let hint = explain(&output.stderr);
        Err(FetchError::Command {
            program: self.program.display().to_string(),
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
        })
    }
}
