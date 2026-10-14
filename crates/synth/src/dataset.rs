//! Pinned Pagila sample database: download, verify, cache.

use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use anyhow::{Context as _, bail};
use sha2::{Digest as _, Sha256};

/// Pagila git commit; newer commits need pgvector and PG18-only features.
pub const PAGILA_COMMIT: &str = "e0e35a666f31a786b9d3c06cb83799fc46db25d4";

/// Distinguishes temp files of concurrent writers within one process.
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// Upper bound for one file download.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// Files to load, in order, with their sha256.
pub const PAGILA_FILES: &[(&str, &str)] = &[
    (
        "pagila-schema.sql",
        "8ce358e4c8014087b85296694a0893887bd7a4190e3ce407f2721b86b98e5707",
    ),
    (
        "pagila-data.sql",
        "880580fb2cd4daaa99f290ced264988cdd657b3158be63cd281466f796f6dbf2",
    ),
];

/// Lowercase hex sha256 of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(64);
    for byte in &Sha256::digest(bytes) {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Checks `bytes` against the expected sha256.
///
/// # Errors
///
/// Fails on mismatch, naming `what`.
pub fn verify(bytes: &[u8], expected: &str, what: &str) -> anyhow::Result<()> {
    let actual = sha256_hex(bytes);
    if actual != expected {
        bail!("{what}: sha256 {actual}, expected {expected}");
    }
    Ok(())
}

/// Writes `bytes` to a sibling temp file, then renames it over `path`.
///
/// # Errors
///
/// Fails if the write or the rename fails.
fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let mut partial = path.as_os_str().to_owned();
    partial.push(format!(
        ".{}.{}.part",
        process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let partial = PathBuf::from(partial);
    fs::write(&partial, bytes).with_context(|| format!("writing {}", partial.display()))?;
    fs::rename(&partial, path).with_context(|| format!("renaming into {}", path.display()))
}

/// Returns the directory with verified Pagila files, downloading missing or corrupt ones.
///
/// # Errors
///
/// Fails on I/O or network errors, or if a download does not match its checksum.
pub fn ensure_pagila(cache: &Path) -> anyhow::Result<PathBuf> {
    let dir = cache.join(format!("pagila-{PAGILA_COMMIT}"));
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(DOWNLOAD_TIMEOUT))
        .build()
        .into();
    for (file, sha) in PAGILA_FILES {
        let path = dir.join(file);
        if fs::read(&path).is_ok_and(|bytes| sha256_hex(&bytes) == *sha) {
            continue;
        }
        let url =
            format!("https://raw.githubusercontent.com/devrimgunduz/pagila/{PAGILA_COMMIT}/{file}");
        let mut response = agent
            .get(&url)
            .call()
            .with_context(|| format!("GET {url}"))?;
        let bytes = response
            .body_mut()
            .read_to_vec()
            .with_context(|| format!("reading body of {url}"))?;
        verify(&bytes, sha, file)?;
        write_atomic(&path, &bytes)?;
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use std::env;

    use super::*;

    #[test]
    fn sha256_hex_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn verify_rejects_mismatch() {
        let good = sha256_hex(b"abc");
        verify(b"abc", &good, "abc").unwrap();
        let err = verify(b"abd", &good, "abc").unwrap_err().to_string();
        assert!(err.contains("abc"), "error should name the file: {err}");
    }

    #[test]
    fn write_atomic_replaces_content_and_leaves_no_partial_file() {
        let dir = env::temp_dir().join(format!("synth-unit-atomic-{}", process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("file.sql");
        std::fs::write(&path, b"old").unwrap();
        write_atomic(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["file.sql"]);
    }

    #[test]
    fn write_atomic_survives_concurrent_writers() {
        let dir = env::temp_dir().join(format!("synth-unit-atomic-race-{}", process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shared.sql");
        let content = vec![b'x'; 1 << 20];
        let results: Vec<_> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..8)
                .map(|_| s.spawn(|| write_atomic(&path, &content)))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for result in &results {
            result.as_ref().unwrap();
        }
        assert_eq!(std::fs::read(&path).unwrap().len(), 1 << 20);
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["shared.sql"]);
    }

    #[test]
    fn pins_two_files_schema_first() {
        assert_eq!(PAGILA_FILES[0].0, "pagila-schema.sql");
        assert_eq!(PAGILA_FILES[1].0, "pagila-data.sql");
        assert!(PAGILA_FILES.iter().all(|(_, sha)| sha.len() == 64));
    }

    #[test]
    #[ignore = "needs network"]
    fn corrupt_cache_file_is_redownloaded() {
        let cache = env::temp_dir().join(format!("synth-unit-pagila-{}", process::id()));
        let _ = fs::remove_dir_all(&cache);
        let dir = ensure_pagila(&cache).unwrap();
        let schema = dir.join(PAGILA_FILES[0].0);
        fs::write(&schema, b"truncated").unwrap();
        ensure_pagila(&cache).unwrap();
        verify(&fs::read(&schema).unwrap(), PAGILA_FILES[0].1, "schema").unwrap();
    }
}
