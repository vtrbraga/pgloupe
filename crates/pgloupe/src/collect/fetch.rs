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

/// Whether process `pid` is running (Linux; elsewhere the lock is trusted).
fn alive(pid: u32) -> bool {
    if cfg!(target_os = "linux") {
        Path::new(&format!("/proc/{pid}")).exists()
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
            if pid != 0 && alive(pid) {
                return Err(FetchError::Io {
                    path: lock_path.display().to_string(),
                    source: std::io::Error::other(format!(
                        "another fetch (process {pid}) is using this directory; if it is not running, delete the file"
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
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_error(&path)(e)),
        }
    }

    fn append(&mut self, name: &str, bytes: &[u8]) -> Result<(), FetchError> {
        let path = self.path(name);
        let mut file: File = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(io_error(&path))?;
        file.write_all(bytes).map_err(io_error(&path))
    }

    fn checkpoint(&mut self, state: &ProviderState) -> Result<(), FetchError> {
        let mut bytes = BTreeMap::new();
        for entry in fs::read_dir(&self.dir)
            .map_err(io_error(&self.dir))?
            .flatten()
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            if let Ok(meta) = entry.metadata()
                && meta.is_file()
            {
                bytes.insert(name, meta.len());
            }
        }
        self.saved = Saved {
            version: 1,
            provider: self.provider.clone(),
            state: state.clone(),
            bytes,
        };
        let path = self.dir.join(STATE_FILE);
        let tmp = self.dir.join(".fetch.json.part");
        let text = serde_json::to_string_pretty(&self.saved).map_err(|e| FetchError::Io {
            path: path.display().to_string(),
            source: std::io::Error::other(e.to_string()),
        })?;
        fs::write(&tmp, text).map_err(io_error(&tmp))?;
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
