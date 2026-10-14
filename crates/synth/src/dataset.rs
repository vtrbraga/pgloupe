//! Pinned Pagila sample database: download, verify, cache.

use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, bail};
use sha2::{Digest as _, Sha256};

/// Pagila git commit; newer commits need pgvector and PG18-only features.
pub const PAGILA_COMMIT: &str = "e0e35a666f31a786b9d3c06cb83799fc46db25d4";

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

/// Returns the directory with verified Pagila files, downloading missing or corrupt ones.
///
/// # Errors
///
/// Fails on I/O or network errors, or if a download does not match its checksum.
pub fn ensure_pagila(cache: &Path) -> anyhow::Result<PathBuf> {
    let dir = cache.join(format!("pagila-{PAGILA_COMMIT}"));
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    for (file, sha) in PAGILA_FILES {
        let path = dir.join(file);
        if fs::read(&path).is_ok_and(|bytes| sha256_hex(&bytes) == *sha) {
            continue;
        }
        let url =
            format!("https://raw.githubusercontent.com/devrimgunduz/pagila/{PAGILA_COMMIT}/{file}");
        let mut response = ureq::get(&url)
            .call()
            .with_context(|| format!("GET {url}"))?;
        let bytes = response
            .body_mut()
            .read_to_vec()
            .with_context(|| format!("reading body of {url}"))?;
        verify(&bytes, sha, file)?;
        fs::write(&path, &bytes).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use std::{env, process};

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
