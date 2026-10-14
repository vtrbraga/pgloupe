//! A reader that hashes the bytes passing through it, for the store's input identity.

use std::{
    fmt,
    io::{self, BufRead, Read},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use xxhash_rust::xxh3::Xxh3;

/// What was read: XXH3-128 of everything, its length, and hashes of requested prefixes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Content {
    pub hash: u128,
    pub size: u64,
    /// `(n, hash of the first n bytes)` for each requested `n` the input reached.
    pub prefixes: Vec<(u64, u128)>,
}

struct State {
    hasher: Xxh3,
    size: u64,
    marks: Vec<u64>,
    next: usize,
    prefixes: Vec<(u64, u128)>,
}

impl State {
    fn feed(&mut self, mut bytes: &[u8]) {
        while let Some(&mark) = self.marks.get(self.next) {
            let Ok(take) = usize::try_from(mark - self.size) else {
                break;
            };
            if take > bytes.len() {
                break;
            }
            let (head, rest) = bytes.split_at(take);
            self.hasher.update(head);
            self.size = mark;
            self.prefixes.push((mark, self.hasher.digest128()));
            self.next += 1;
            bytes = rest;
        }
        self.hasher.update(bytes);
        self.size += bytes.len() as u64;
    }
}

fn locked(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A reader that feeds everything consumed from it to a hasher; see [`tee`].
pub struct Tee {
    inner: Box<dyn BufRead + Send>,
    state: Arc<Mutex<State>>,
}

impl fmt::Debug for Tee {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tee")
            .field("size", &locked(&self.state).size)
            .finish_non_exhaustive()
    }
}

/// Reads the result of a [`Tee`] after the reader has been consumed.
#[derive(Clone)]
pub struct TeeHandle(Arc<Mutex<State>>);

impl fmt::Debug for TeeHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TeeHandle")
            .field("size", &locked(&self.0).size)
            .finish_non_exhaustive()
    }
}

impl TeeHandle {
    /// The hash, length and prefix hashes of the bytes consumed so far.
    #[must_use]
    pub fn finish(&self) -> Content {
        let state = locked(&self.0);
        Content {
            hash: state.hasher.digest128(),
            size: state.size,
            prefixes: state.prefixes.clone(),
        }
    }
}

/// Wraps `inner` so that every byte read through it is hashed. `marks` are input lengths at which
/// to snapshot the running hash; they are sorted, deduplicated, and zero is dropped.
#[must_use]
pub fn tee(inner: Box<dyn BufRead + Send>, mut marks: Vec<u64>) -> (Tee, TeeHandle) {
    marks.retain(|mark| *mark > 0);
    marks.sort_unstable();
    marks.dedup();
    let state = Arc::new(Mutex::new(State {
        hasher: Xxh3::new(),
        size: 0,
        marks,
        next: 0,
        prefixes: Vec::new(),
    }));
    let handle = TeeHandle(Arc::clone(&state));
    (Tee { inner, state }, handle)
}

impl Read for Tee {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        locked(&self.state).feed(&buf[..n]);
        Ok(n)
    }
}

impl BufRead for Tee {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.inner.fill_buf()
    }

    fn consume(&mut self, amount: usize) {
        // Bytes are hashed when consumed, not when peeked. A `fill_buf` right before `consume`
        // returns the same buffered slice without reading more.
        if let Ok(buf) = self.inner.fill_buf() {
            let n = amount.min(buf.len());
            locked(&self.state).feed(&buf[..n]);
        }
        self.inner.consume(amount);
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use xxhash_rust::xxh3::xxh3_128;

    use super::*;
    use crate::log::source::open_reader;

    const DATA: &[u8] = b"line one\nline two\nline three\n";

    fn through_bufread(data: &[u8], marks: Vec<u64>) -> Content {
        let reader = open_reader(Cursor::new(data.to_vec())).unwrap();
        let (mut tee, handle) = tee(reader, marks);
        let mut line = String::new();
        while tee.read_line(&mut line).unwrap() > 0 {
            line.clear();
        }
        handle.finish()
    }

    #[test]
    fn hashes_what_the_reader_consumed() {
        let content = through_bufread(DATA, vec![]);
        assert_eq!(content.hash, xxh3_128(DATA));
        assert_eq!(content.size, DATA.len() as u64);
    }

    #[test]
    fn read_and_bufread_paths_agree() {
        let reader = open_reader(Cursor::new(DATA.to_vec())).unwrap();
        let (mut tee, handle) = tee(reader, vec![]);
        let mut sink = Vec::new();
        tee.read_to_end(&mut sink).unwrap();
        assert_eq!(handle.finish().hash, xxh3_128(DATA));
    }

    #[test]
    fn snapshots_prefix_hashes_at_the_marks() {
        let content = through_bufread(DATA, vec![9, 18, 1_000]);
        assert_eq!(
            content.prefixes,
            vec![(9, xxh3_128(&DATA[..9])), (18, xxh3_128(&DATA[..18]))]
        );
    }

    #[test]
    fn unsorted_duplicate_and_zero_marks_are_harmless() {
        let content = through_bufread(DATA, vec![18, 0, 9, 9]);
        assert_eq!(content.prefixes.len(), 2);
    }

    #[test]
    fn a_mark_at_the_very_end_is_the_whole_hash() {
        let end = DATA.len() as u64;
        let content = through_bufread(DATA, vec![end]);
        assert_eq!(content.prefixes, vec![(end, content.hash)]);
    }

    #[test]
    fn gzip_and_plain_have_the_same_identity() {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(DATA).unwrap();
        let gz = encoder.finish().unwrap();
        assert_eq!(
            through_bufread(&gz, vec![]).hash,
            through_bufread(DATA, vec![]).hash
        );
    }
}
