//! Run manifest: what was generated and which log events to expect.

use std::{fs, path::Path};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

/// Manifest file name inside a run directory.
pub const FILE_NAME: &str = "manifest.json";

/// Kind of log event a scenario is expected to produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// `duration: … statement: …` (simple protocol).
    Statement,
    /// `duration: … parse …` (extended protocol).
    Parse,
    /// `duration: … bind …` (extended protocol).
    Bind,
    /// `duration: … execute …` (extended protocol).
    Execute,
    /// `… still waiting for … after …`.
    LockWait,
    /// `… acquired … after …`.
    LockAcquired,
    /// ERROR with SQLSTATE `40P01`.
    Deadlock,
    /// `temporary file: …`.
    TempFile,
    /// `checkpoint starting: immediate force wait`; not attributed to an application.
    Checkpoint,
    /// `connection authorized: …`; application read from the message.
    ConnectionAuthorized,
    /// `disconnection: …`.
    Disconnection,
    /// ERROR with the SQLSTATE given in [`Expect::sqlstate`].
    Error,
}

/// Expected count of one event kind for one scenario.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Expect {
    /// Scenario name.
    pub scenario: String,
    /// `application_name` of the scenario's sessions.
    pub app: String,
    /// Event kind.
    pub event: EventKind,
    /// SQLSTATE for [`EventKind::Error`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sqlstate: Option<String>,
    /// Minimum count.
    pub min: u32,
    /// Maximum count; `None` means unbounded.
    pub max: Option<u32>,
}

/// Everything a test needs to know about a generated corpus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// `synth` crate version.
    pub generator_version: String,
    /// Postgres major version requested.
    pub pg_major: u32,
    /// `server_version_num` reported by the server.
    pub server_version_num: i32,
    /// Docker image tag.
    pub image: String,
    /// Image repo digest.
    pub image_digest: String,
    /// Workload seed.
    pub seed: u64,
    /// Pagila git commit.
    pub pagila_commit: String,
    /// Server settings passed at startup.
    pub settings: Vec<(String, String)>,
    /// Server time when loading finished (`now()::text`).
    pub load_finished_at: String,
    /// Server time of snapshots 0, 1 and 2.
    pub snapshots_at: Vec<String>,
    /// Scenarios run, in order.
    pub scenarios: Vec<String>,
    /// Exact statement texts sent by the `multiline` scenario.
    pub multiline_texts: Vec<String>,
    /// Expected log events.
    pub expect: Vec<Expect>,
}

impl Manifest {
    /// Writes the manifest as pretty JSON to `dir/manifest.json`.
    ///
    /// # Errors
    ///
    /// Fails if serialization or the write fails.
    pub fn write(&self, dir: &Path) -> anyhow::Result<()> {
        let path = dir.join(FILE_NAME);
        let json = serde_json::to_string_pretty(self)?;
        fs::write(&path, json).with_context(|| format!("writing {}", path.display()))
    }

    /// Reads `dir/manifest.json`.
    ///
    /// # Errors
    ///
    /// Fails if the file is missing or is not a valid manifest.
    pub fn read(dir: &Path) -> anyhow::Result<Self> {
        let path = dir.join(FILE_NAME);
        let json =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&json).with_context(|| format!("parsing {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use std::{env, path::PathBuf, process};

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("synth-unit-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample() -> Manifest {
        Manifest {
            generator_version: "0.0.0".into(),
            pg_major: 17,
            server_version_num: 170_003,
            image: "postgres:17".into(),
            image_digest: "postgres@sha256:abc".into(),
            seed: 42,
            pagila_commit: "e0e35a6".into(),
            settings: vec![("jit".into(), "off".into())],
            load_finished_at: "2026-10-13 12:00:00+00".into(),
            snapshots_at: vec!["2026-10-13 12:00:01+00".into()],
            scenarios: vec!["errors".into()],
            multiline_texts: vec!["SELECT\n1".into()],
            expect: vec![Expect {
                scenario: "errors".into(),
                app: "synth:errors".into(),
                event: EventKind::Error,
                sqlstate: Some("22012".into()),
                min: 1,
                max: Some(1),
            }],
        }
    }

    #[test]
    fn manifest_round_trips_through_disk() {
        let dir = scratch("manifest-rt");
        let manifest = sample();
        manifest.write(&dir).unwrap();
        assert_eq!(Manifest::read(&dir).unwrap(), manifest);
    }

    #[test]
    fn event_kinds_serialize_as_snake_case() {
        assert_eq!(
            serde_json::to_string(&EventKind::LockWait).unwrap(),
            "\"lock_wait\""
        );
        assert_eq!(
            serde_json::to_string(&EventKind::ConnectionAuthorized).unwrap(),
            "\"connection_authorized\""
        );
    }

    #[test]
    fn missing_manifest_is_an_error() {
        let dir = scratch("manifest-missing");
        Manifest::read(&dir).unwrap_err();
    }
}
