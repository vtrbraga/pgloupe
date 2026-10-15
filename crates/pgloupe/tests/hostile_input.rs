//! Feeds damaged copies of every log fixture to every decoder: nothing may panic or hang.

use std::{fs, io, io::Cursor, path::PathBuf};

use pgloupe::log::{Format, Options, decoder_for};

const PREFIX: &str = "%m [%p] %q%u@%d/%a %Q ";

fn fixtures() -> io::Result<Vec<(String, Vec<u8>)>> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/logs");
    let mut found: Vec<(String, Vec<u8>)> = fs::read_dir(dir)?
        .flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| {
            Some((
                e.file_name().to_string_lossy().into_owned(),
                fs::read(e.path()).ok()?,
            ))
        })
        .collect();
    found.sort();
    assert!(found.len() >= 8, "{} fixtures", found.len());
    Ok(found)
}

/// A small deterministic generator: the same damage on every run.
struct Xorshift(u64);

impl Xorshift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(n.max(1)).unwrap_or(1)).unwrap_or(0)
    }
}

/// The damage: cuts at several points, flipped bytes, NULs, a very long line, CR for LF.
fn damaged(original: &[u8], seed: u64) -> Vec<Vec<u8>> {
    let mut rng = Xorshift(seed | 1);
    let len = original.len();
    let mut out = Vec::new();
    for cut in [0, 1, len / 3, len / 2, len.saturating_sub(1)] {
        out.push(original[..cut.min(len)].to_vec());
    }
    for _ in 0..4 {
        let mut copy = original.to_vec();
        for _ in 0..(len / 50).max(1) {
            if copy.is_empty() {
                break;
            }
            let at = rng.below(copy.len());
            copy[at] ^= 1 << rng.below(8);
        }
        out.push(copy);
    }
    let mut nul = original.to_vec();
    for _ in 0..5 {
        if !nul.is_empty() {
            let at = rng.below(nul.len());
            nul[at] = 0;
        }
    }
    out.push(nul);
    let mut long = original.to_vec();
    let at = rng.below(long.len().max(1));
    long.splice(at..at, std::iter::repeat_n(b'x', 300_000));
    out.push(long);
    out.push(
        original
            .iter()
            .map(|b| if *b == b'\n' { b'\r' } else { *b })
            .collect(),
    );
    out
}

fn decode_everything(format: Format, bytes: Vec<u8>) {
    let options = Options {
        prefix: Some(PREFIX.to_owned()),
        log_tz: None,
    };
    let Ok(mut decoder) = decoder_for(format, Box::new(Cursor::new(bytes)), &options) else {
        return;
    };
    // Reading to the end must terminate; errors are fine, panics are not.
    while let Ok(Some(_)) = decoder.next_record() {}
}

#[test]
fn damaged_logs_never_panic_any_decoder() -> io::Result<()> {
    for (name, original) in fixtures()? {
        for (index, bytes) in damaged(&original, 0x9e37_79b9 ^ original.len() as u64)
            .into_iter()
            .enumerate()
        {
            for format in [Format::Stderr, Format::Csv, Format::Json, Format::Pgbouncer] {
                let label = format!("{name} damage {index} as {format:?}");
                let result = std::panic::catch_unwind(|| decode_everything(format, bytes.clone()));
                assert!(result.is_ok(), "{label} panicked");
            }
        }
    }
    Ok(())
}
