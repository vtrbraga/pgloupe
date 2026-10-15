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

/// The most a tool may print on standard output; more is a failure, not a bigger buffer.
const STDOUT_LIMIT: u64 = 256 << 20;

/// The most of standard error that is read.
const STDERR_LIMIT: u64 = 1 << 20;

/// `ETXTBSY`: a program file that something still has open for writing.
const TEXT_FILE_BUSY: i32 = 26;

/// Starts `command`. A program that was written an instant ago can fail with "text file busy"
/// while another thread's `fork` still holds the file open; that clears in milliseconds.
fn spawn_retrying(command: &mut Command) -> std::io::Result<std::process::Child> {
    let mut attempts = 0;
    loop {
        match command.spawn() {
            Err(error) if error.raw_os_error() == Some(TEXT_FILE_BUSY) && attempts < 10 => {
                attempts += 1;
                thread::sleep(Duration::from_millis(25));
            }
            other => return other,
        }
    }
}

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
        self.run_in(args, None)
    }

    /// Like [`Tool::run`], in directory `cwd` (for tools that write their downloads there).
    ///
    /// # Errors
    ///
    /// Fails like [`Tool::run`].
    pub fn run_in(
        &self,
        args: &[String],
        cwd: Option<&std::path::Path>,
    ) -> Result<Output, FetchError> {
        let shown = self.program.display().to_string();
        let mut command = Command::new(&self.program);
        command.args(args).envs(self.env.iter().copied());
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = spawn_retrying(&mut command).map_err(|error| {
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
        let over = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let over_in_reader = std::sync::Arc::clone(&over);
        let stdout_reader = thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(out) = out.as_mut() {
                drop(out.take(STDOUT_LIMIT + 1).read_to_end(&mut buf));
            }
            if buf.len() as u64 > STDOUT_LIMIT {
                over_in_reader.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            buf
        });
        let stderr_reader = thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(err) = err.as_mut() {
                drop(err.take(STDERR_LIMIT).read_to_end(&mut buf));
            }
            buf
        });
        let deadline = Instant::now() + Duration::from_secs(self.timeout_secs);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if over.load(std::sync::atomic::Ordering::Relaxed) => {
                    drop(child.kill());
                    drop(child.wait());
                    return Err(FetchError::Command {
                        program: shown,
                        command: String::new(),
                        status: format!("printed more than {} MB", STDOUT_LIMIT >> 20),
                        stderr: String::new(),
                        hint: String::new(),
                    });
                }
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
