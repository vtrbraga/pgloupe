//! Getting a time range of log text out of a vendor, through the vendor's command-line tool.

pub mod aiven;
pub mod aws;
pub mod azure;
pub mod cloudwatch;
pub mod cmd;
pub mod entries;
pub mod gcp;
pub mod heroku;
pub mod sql;

use std::collections::BTreeMap;

use jiff::Timestamp;
use thiserror::Error;

/// Why a fetch step failed. Messages say which tool and what to try.
#[derive(Debug, Error)]
pub enum FetchError {
    /// The vendor tool is not installed.
    #[error("`{program}` was not found: {hint}")]
    MissingCli {
        /// The program looked for.
        program: String,
        /// How to install or point at it.
        hint: &'static str,
    },
    /// The tool ran and failed.
    #[error("`{program} {command}` failed ({status}): {stderr}{hint}")]
    Command {
        /// The program.
        program: String,
        /// The subcommand, without secrets.
        command: String,
        /// Exit status.
        status: String,
        /// What it printed on stderr, cut.
        stderr: String,
        /// What to try, or empty.
        hint: String,
    },
    /// The tool's output was not what the vendor documents.
    #[error("{provider}: unexpected output from {what}: {detail}")]
    Output {
        /// The provider.
        provider: &'static str,
        /// Which call.
        what: &'static str,
        /// What was wrong.
        detail: String,
    },
    /// The tool took too long.
    #[error("`{program}` did not finish within {seconds} s")]
    Timeout {
        /// The program.
        program: String,
        /// The limit.
        seconds: u64,
    },
    /// The destination could not be written.
    #[error("{path}: {source}")]
    Io {
        /// The file.
        path: String,
        /// The cause.
        #[source]
        source: std::io::Error,
    },
}

/// What to fetch.
#[derive(Debug, Clone)]
pub struct Request {
    /// Log content written at or after this time.
    pub since: Timestamp,
    /// Only content before this time, where the vendor lets us say so.
    pub until: Option<Timestamp>,
}

/// Where fetched text goes. Files are append-only.
pub trait LogSink {
    /// Bytes already stored under `name` (0 if none).
    fn stored(&self, name: &str) -> u64;
    /// Forgets `name` so it is written from the start (the vendor's file shrank or rotated).
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be removed.
    fn reset(&mut self, name: &str) -> Result<(), FetchError>;
    /// Adds `bytes` to the end of `name`.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be written.
    fn append(&mut self, name: &str, bytes: &[u8]) -> Result<(), FetchError>;
    /// Saves `state` together with the current size of every file, so a later run can continue
    /// from exactly here (and drop anything written after it by an interrupted run).
    ///
    /// # Errors
    ///
    /// Fails if the state cannot be written.
    fn checkpoint(&mut self, state: &ProviderState) -> Result<(), FetchError>;
}

/// What a provider remembers between runs (markers, timestamps), by key.
pub type ProviderState = BTreeMap<String, String>;

/// What a fetch did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    /// Files written to or created.
    pub files: Vec<String>,
    /// Bytes added.
    pub bytes: u64,
    /// Files that were started again because the vendor's copy shrank.
    pub restarted: Vec<String>,
}

/// One vendor.
pub trait LogProvider {
    /// The name used on the command line.
    fn name(&self) -> &'static str;
    /// Whether the connector has not been confirmed against the real service. New providers are
    /// experimental until their author says otherwise.
    fn experimental(&self) -> bool {
        true
    }
    /// The tool is installed and its credentials work; the error says how to fix it if not.
    ///
    /// # Errors
    ///
    /// Fails with the fix when the tool is missing or not signed in.
    fn check(&self) -> Result<(), FetchError>;
    /// Writes log text for `request` into `sink`, continuing from `state`.
    ///
    /// # Errors
    ///
    /// Fails when the tool fails or answers with something unexpected; what was written stays.
    fn fetch(
        &self,
        request: &Request,
        sink: &mut dyn LogSink,
        state: &mut ProviderState,
    ) -> Result<Summary, FetchError>;
}
