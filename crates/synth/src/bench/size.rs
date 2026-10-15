//! Human sizes (`1G`) and the size of a growing log directory.

use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{Context as _, bail};

use super::truth::FormatSize;

/// Parses `1G`, `256M`, `512K` or plain bytes; powers of 1024.
///
/// # Errors
///
/// Fails on an empty string, a missing number or an unknown suffix.
pub fn parse_size(text: &str) -> anyhow::Result<u64> {
    let digits = text.trim_end_matches(|c: char| !c.is_ascii_digit());
    let suffix = &text[digits.len()..];
    let shift = match suffix.to_ascii_lowercase().as_str() {
        "" => 0,
        "k" => 10,
        "m" => 20,
        "g" => 30,
        _ => bail!("unknown size suffix in {text:?}; use K, M or G"),
    };
    let number: u64 = digits
        .parse()
        .with_context(|| format!("{text:?} is not a size like 1G"))?;
    number
        .checked_mul(1 << shift)
        .with_context(|| format!("{text:?} is too large"))
}

/// Files and bytes per log format (`log`, `csv`, `json`) in `dir`. A missing directory is empty.
///
/// # Errors
///
/// Fails if the directory exists but cannot be read.
pub fn per_format(dir: &Path) -> anyhow::Result<BTreeMap<String, FormatSize>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };
    let mut sizes: BTreeMap<String, FormatSize> = BTreeMap::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
        let path = entry.path();
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if matches!(ext, "log" | "csv" | "json") {
            let len = entry
                .metadata()
                .with_context(|| format!("reading {}", path.display()))?
                .len();
            let slot = sizes.entry(ext.to_owned()).or_default();
            slot.files += 1;
            slot.bytes += len;
        }
    }
    Ok(sizes)
}

/// Bytes of the largest log format in `dir`: the sum of its files, maximum over formats.
///
/// # Errors
///
/// Fails if the directory exists but cannot be read.
pub fn log_bytes(dir: &Path) -> anyhow::Result<u64> {
    Ok(per_format(dir)?
        .values()
        .map(|f| f.bytes)
        .max()
        .unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use std::{env, process};

    use super::*;

    #[test]
    fn sizes_use_powers_of_1024() {
        assert_eq!(parse_size("1G").unwrap(), 1 << 30);
        assert_eq!(parse_size("256M").unwrap(), 256 << 20);
        assert_eq!(parse_size("10g").unwrap(), 10 << 30);
        assert_eq!(parse_size("512K").unwrap(), 512 << 10);
        assert_eq!(parse_size("1024").unwrap(), 1024);
    }

    #[test]
    fn bad_sizes_are_rejected() {
        for text in ["", "G", "1X", "-1G", "1.5G", "1 G"] {
            parse_size(text).unwrap_err();
        }
    }

    #[test]
    fn the_largest_format_counts() {
        let dir = env::temp_dir().join(format!("synth-size-{}", process::id()));
        fs::create_dir_all(&dir).unwrap();
        for (name, len) in [
            ("a.log", 10),
            ("a.csv", 30),
            ("a.json", 20),
            ("b.json", 5),
            ("x.txt", 99),
        ] {
            fs::write(dir.join(name), vec![b'x'; len]).unwrap();
        }
        assert_eq!(log_bytes(&dir).unwrap(), 30);
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(log_bytes(&dir).unwrap(), 0);
    }
}
