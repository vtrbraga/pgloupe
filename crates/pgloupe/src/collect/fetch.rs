//! Fetching into a directory: append-only files, a state file, and a lock.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::provider::{FetchError, LogSink, ProviderState};

/// The state file name inside the directory.
pub const STATE_FILE: &str = ".fetch.json";
/// The lock file name inside the directory.
pub const LOCK_FILE: &str = ".fetch.lock";

#[derive(Debug, Default, Serialize, Deserialize)]
struct Saved {
    version: u32,
    provider: String,
    state: ProviderState,
    /// Size of each file when the state was saved.
    bytes: BTreeMap<String, u64>,
}

/// Holds the directory for one fetch at a time; released on drop.
#[derive(Debug)]
pub struct Lock {
    path: PathBuf,
}

impl Drop for Lock {
    fn drop(&mut self) {
        drop(fs::remove_file(&self.path));
    }
}

fn io_error(path: &Path) -> impl FnOnce(std::io::Error) -> FetchError + '_ {
    move |source| FetchError::Io {
        path: path.display().to_string(),
        source,
    }
}

/// Whether process `pid` is running (`/proc` on Linux, `kill -0` on other Unix systems; on
/// Windows the lock is trusted).
fn alive(pid: u32) -> bool {
    if cfg!(target_os = "linux") {
        Path::new(&format!("/proc/{pid}")).exists()
    } else if cfg!(unix) {
        // A failure to run `kill` at all leaves the lock trusted.
        match std::process::Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stderr(std::process::Stdio::null())
            .status()
        {
            Ok(status) => status.success(),
            Err(_) => true,
        }
    } else {
        true
    }
}

/// A directory fetched into.
#[derive(Debug)]
pub struct DirSink {
    dir: PathBuf,
    provider: String,
    saved: Saved,
    _lock: Lock,
}

impl DirSink {
    /// Opens `dir` (creating it), takes its lock, loads the state saved by an earlier run for the
    /// same provider, and cuts any file back to the size it had when that state was saved.
    ///
    /// # Errors
    ///
    /// Fails if another fetch holds the directory, the state belongs to another provider, or a
    /// file cannot be read or cut.
    pub fn open(dir: &Path, provider: &str) -> Result<DirSink, FetchError> {
        fs::create_dir_all(dir).map_err(io_error(dir))?;
        let lock_path = dir.join(LOCK_FILE);
        if let Ok(text) = fs::read_to_string(&lock_path) {
            let pid: u32 = text.trim().parse().unwrap_or(0);
            // A lock with no process id is one whose owner has not written it yet, unless it is
            // old: then its owner died between creating and writing it.
            let being_written = pid == 0
                && fs::metadata(&lock_path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age < std::time::Duration::from_secs(5));
            if being_written || (pid != 0 && alive(pid)) {
                return Err(FetchError::Io {
                    path: lock_path.display().to_string(),
                    source: std::io::Error::other(format!(
                        "another fetch (process {}) is using this directory; if it is not running, delete the file",
                        if pid == 0 {
                            "starting".to_owned()
                        } else {
                            pid.to_string()
                        }
                    )),
                });
            }
            drop(fs::remove_file(&lock_path));
        }
        let mut lock_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
            .map_err(io_error(&lock_path))?;
        write!(lock_file, "{}", std::process::id()).map_err(io_error(&lock_path))?;
        let lock = Lock { path: lock_path };
        let state_path = dir.join(STATE_FILE);
        let saved: Saved = match fs::read_to_string(&state_path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| FetchError::Io {
                path: state_path.display().to_string(),
                source: std::io::Error::other(format!("not a valid state file: {e}")),
            })?,
            Err(_) => Saved::default(),
        };
        if !saved.provider.is_empty() && saved.provider != provider {
            return Err(FetchError::Io {
                path: state_path.display().to_string(),
                source: std::io::Error::other(format!(
                    "this directory was fetched with provider {}, not {provider}; use another directory",
                    saved.provider
                )),
            });
        }
        for (name, bytes) in &saved.bytes {
            let path = dir.join(name);
            if let Ok(meta) = fs::metadata(&path)
                && meta.len() > *bytes
            {
                // An interrupted run wrote past its last checkpoint; drop the excess.
                let file = OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .map_err(io_error(&path))?;
                file.set_len(*bytes).map_err(io_error(&path))?;
            }
        }
        Ok(DirSink {
            dir: dir.to_owned(),
            provider: provider.to_owned(),
            saved,
            _lock: lock,
        })
    }

    /// What the provider saved last time.
    #[must_use]
    pub fn state(&self) -> ProviderState {
        self.saved.state.clone()
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

impl LogSink for DirSink {
    fn stored(&self, name: &str) -> u64 {
        fs::metadata(self.path(name)).map_or(0, |m| m.len())
    }

    fn reset(&mut self, name: &str) -> Result<(), FetchError> {
        let path = self.path(name);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_error(&path)(e)),
        }
        // Claimed at zero, so a crash before the next checkpoint cuts the file back to nothing.
        self.saved.bytes.insert(name.to_owned(), 0);
        self.persist()
    }

    fn append(&mut self, name: &str, bytes: &[u8]) -> Result<(), FetchError> {
        if !self.saved.bytes.contains_key(name) {
            // Record ownership before the first byte lands, so a crash cannot leave an
            // unaccounted file that the next run would append to a second time.
            self.saved.bytes.insert(name.to_owned(), 0);
            self.persist()?;
        }
        let path = self.path(name);
        let mut file: File = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(io_error(&path))?;
        file.write_all(bytes).map_err(io_error(&path))
    }

    fn checkpoint(&mut self, state: &ProviderState) -> Result<(), FetchError> {
        // Only files this fetch wrote are tracked; anything else in the directory is left alone.
        let mut bytes = BTreeMap::new();
        for name in self.saved.bytes.keys() {
            if let Ok(meta) = fs::metadata(self.dir.join(name)) {
                bytes.insert(name.clone(), meta.len());
            }
        }
        self.saved = Saved {
            version: 1,
            provider: self.provider.clone(),
            state: state.clone(),
            bytes,
        };
        self.persist()
    }
}

impl DirSink {
    fn persist(&self) -> Result<(), FetchError> {
        let path = self.dir.join(STATE_FILE);
        let tmp = self.dir.join(".fetch.json.part");
        let text = serde_json::to_string_pretty(&self.saved).map_err(|e| FetchError::Io {
            path: path.display().to_string(),
            source: std::io::Error::other(e.to_string()),
        })?;
        let mut file = File::create(&tmp).map_err(io_error(&tmp))?;
        file.write_all(text.as_bytes()).map_err(io_error(&tmp))?;
        // On disk before it replaces the old state, or a crash could leave an empty one.
        file.sync_all().map_err(io_error(&tmp))?;
        fs::rename(&tmp, &path).map_err(io_error(&path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pgloupe-fetchdir-{}-{name}", std::process::id()));
        drop(fs::remove_dir_all(&dir));
        dir
    }

    #[test]
    fn a_second_fetch_in_the_same_directory_is_refused_while_the_first_runs() {
        let d = dir("lock");
        let first = DirSink::open(&d, "aws-rds").unwrap();
        let err = DirSink::open(&d, "aws-rds").unwrap_err().to_string();
        assert!(err.contains("another fetch"), "{err}");
        drop(first);
        DirSink::open(&d, "aws-rds").unwrap();
    }

    #[test]
    fn a_lock_with_no_process_id_yet_is_a_fetch_that_is_starting() {
        let d = dir("starting");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join(LOCK_FILE), "").unwrap();
        let err = DirSink::open(&d, "aws-rds").unwrap_err().to_string();
        assert!(err.contains("another fetch"), "{err}");
    }

    #[test]
    fn a_stale_lock_from_a_dead_process_is_replaced() {
        let d = dir("stale");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join(LOCK_FILE), "4294967").unwrap();
        DirSink::open(&d, "aws-rds").unwrap();
    }

    #[test]
    fn bytes_written_after_the_last_checkpoint_are_dropped_on_the_next_open() {
        let d = dir("cut");
        let mut sink = DirSink::open(&d, "aws-rds").unwrap();
        sink.append("a", b"kept\n").unwrap();
        let mut state = ProviderState::new();
        state.insert("marker:a".to_owned(), "1:5".to_owned());
        sink.checkpoint(&state).unwrap();
        sink.append("a", b"half a line that was never checkpointed")
            .unwrap();
        drop(sink);
        let sink = DirSink::open(&d, "aws-rds").unwrap();
        assert_eq!(fs::read(d.join("a")).unwrap(), b"kept\n");
        assert_eq!(sink.state()["marker:a"], "1:5");
        assert_eq!(sink.stored("a"), 5);
    }

    #[test]
    fn a_new_file_left_by_a_crashed_run_is_cut_to_nothing_on_the_next_open() {
        let d = dir("newfile");
        let mut sink = DirSink::open(&d, "aws-rds").unwrap();
        sink.append("fresh", b"half of a file").unwrap();
        drop(sink);
        DirSink::open(&d, "aws-rds").unwrap();
        assert_eq!(fs::read(d.join("fresh")).unwrap(), b"");
    }

    #[test]
    fn files_the_fetch_never_wrote_are_left_alone() {
        let d = dir("foreign");
        let mut sink = DirSink::open(&d, "aws-rds").unwrap();
        fs::write(d.join("notes.txt"), "short").unwrap();
        sink.checkpoint(&ProviderState::new()).unwrap();
        fs::write(d.join("notes.txt"), "much longer user text").unwrap();
        drop(sink);
        DirSink::open(&d, "aws-rds").unwrap();
        assert_eq!(
            fs::read_to_string(d.join("notes.txt")).unwrap(),
            "much longer user text"
        );
    }

    #[test]
    fn a_directory_belongs_to_one_provider() {
        let d = dir("owner");
        let mut sink = DirSink::open(&d, "aws-rds").unwrap();
        sink.checkpoint(&ProviderState::new()).unwrap();
        drop(sink);
        let err = DirSink::open(&d, "azure-flexible").unwrap_err().to_string();
        assert!(err.contains("another directory"), "{err}");
    }

    #[test]
    fn reset_forgets_a_file_and_missing_files_are_fine() {
        let d = dir("reset");
        let mut sink = DirSink::open(&d, "aws-rds").unwrap();
        sink.append("x", b"abc").unwrap();
        sink.reset("x").unwrap();
        assert_eq!(sink.stored("x"), 0);
        sink.reset("never-existed").unwrap();
    }
}
