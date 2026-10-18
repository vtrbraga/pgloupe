//! Reading one stderr file on several threads.
//!
//! What a line says by itself (its bytes, its UTF-8, its prefix fields and label) depends on no
//! other line, so threads parse blocks of lines while the decoder, in order, groups the parsed
//! lines into entries. The decoder sees the same lines in the same order as when it reads and
//! parses them itself, so the records and the statistics are the same.

use std::{
    collections::BTreeMap,
    fmt,
    io::BufRead,
    sync::{Condvar, Mutex, MutexGuard, PoisonError, TryLockError},
    time::Duration,
};

use jiff::{Timestamp, tz::Offset};

use super::{
    DecodeError,
    line::{Line, read_line_capped},
    prefix::{Label, Prefix},
    record::{LogRecord, Severity},
};

/// Bytes of lines in a block, more or less: a block ends with the line that reaches this size.
pub const BLOCK_BYTES: usize = 128 * 1024;

/// What the line-local work made of one physical line.
#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "almost every line is an entry; boxing its record would add an allocation to each"
)]
pub(super) enum Parsed {
    /// Nothing but a line break.
    Blank,
    /// Longer than the decoders keep.
    TooLong,
    /// Not UTF-8; `pid` is read from what can be made of it.
    BadUtf8 { pid: Option<u32> },
    /// Extends the entry before it.
    Tab,
    /// Any other line: its prefix matched (the label, and where its text starts) or the reason it
    /// did not. The record holds the prefix fields that were read.
    Entry {
        parsed: Result<(Label, usize), String>,
        record: Option<LogRecord>,
    },
}

/// A physical line of a [`ParsedBlock`]: where its text is, and what it is.
#[derive(Debug)]
pub(super) struct Item {
    pub start: usize,
    pub end: usize,
    pub what: Parsed,
}

/// A run of consecutive lines, parsed.
#[derive(Debug)]
pub(super) struct ParsedBlock {
    pub text: String,
    pub items: Vec<Item>,
    /// A read error that came after the last of these lines.
    pub error: Option<DecodeError>,
}

/// The bytes of a run of lines, as read.
struct RawBlock {
    bytes: Vec<u8>,
    /// `(start, end, too_long)` of each line; a too long line keeps no bytes.
    lines: Vec<(usize, usize, bool)>,
    error: Option<DecodeError>,
    /// Nothing follows this block.
    last: bool,
}

fn read_block(mut reader: &mut dyn BufRead, target: usize, max_line: usize) -> RawBlock {
    let mut block = RawBlock {
        bytes: Vec::with_capacity(target + 4096),
        lines: Vec::new(),
        error: None,
        last: false,
    };
    let mut line = Vec::new();
    while block.bytes.len() < target {
        match read_line_capped(&mut reader, &mut line, max_line) {
            Ok(Line::End) => {
                block.last = true;
                break;
            }
            Ok(Line::Complete) => {
                let start = block.bytes.len();
                block.bytes.extend_from_slice(&line);
                block.lines.push((start, block.bytes.len(), false));
            }
            Ok(Line::TooLong) => {
                let at = block.bytes.len();
                block.lines.push((at, at, true));
            }
            Err(error) => {
                block.error = Some(DecodeError::from(error));
                block.last = true;
                break;
            }
        }
    }
    block
}

/// The line without its trailing carriage returns.
fn trimmed(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    while end > 0 && matches!(line[end - 1], b'\n' | b'\r') {
        end -= 1;
    }
    &line[..end]
}

fn parse_block(
    raw: RawBlock,
    first: bool,
    prefix: &Prefix,
    fallback: Option<Offset>,
) -> ParsedBlock {
    let RawBlock {
        bytes,
        lines,
        error,
        ..
    } = raw;
    let mut text = String::with_capacity(bytes.len());
    let mut items = Vec::with_capacity(lines.len());
    for (number, &(start, end, too_long)) in lines.iter().enumerate() {
        if too_long {
            items.push(Item {
                start: text.len(),
                end: text.len(),
                what: Parsed::TooLong,
            });
            continue;
        }
        let line = trimmed(&bytes[start..end]);
        let Ok(valid) = std::str::from_utf8(line) else {
            // The pid usually sits before the broken bytes: use it to close the right entry.
            let lossy = String::from_utf8_lossy(line);
            let mut record = LogRecord::new(Timestamp::UNIX_EPOCH, Severity::Log, String::new());
            drop(prefix.parse_line(&lossy, &mut record, fallback));
            items.push(Item {
                start: text.len(),
                end: text.len(),
                what: Parsed::BadUtf8 { pid: record.pid },
            });
            continue;
        };
        // The first line of the file may start with a byte order mark.
        let valid = if first && number == 0 {
            valid.trim_start_matches('\u{feff}')
        } else {
            valid
        };
        let at = text.len();
        text.push_str(valid);
        let what = if valid.is_empty() {
            Parsed::Blank
        } else if valid.starts_with('\t') {
            Parsed::Tab
        } else {
            let mut record = LogRecord::new(Timestamp::UNIX_EPOCH, Severity::Log, String::new());
            let parsed = prefix
                .parse_line(valid, &mut record, fallback)
                .map(|(label, rest)| (label, valid.len() - rest.len()));
            Parsed::Entry {
                parsed,
                record: Some(record),
            }
        };
        items.push(Item {
            start: at,
            end: text.len(),
            what,
        });
    }
    ParsedBlock { text, items, error }
}

#[derive(Debug)]
struct Raw {
    reader: ReaderBox,
    /// Index of the next block to read.
    next: usize,
    done: bool,
}

struct ReaderBox(Box<dyn BufRead + Send>);

impl fmt::Debug for ReaderBox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReaderBox")
    }
}

#[derive(Debug, Default)]
struct Ready {
    blocks: BTreeMap<usize, ParsedBlock>,
    /// Index of the block the decoder wants next.
    wanted: usize,
    /// How many blocks the file has, once all are read.
    total: Option<usize>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Blocks of one file, read in order and parsed by whichever thread is free.
#[derive(Debug)]
pub struct BlockFeed {
    prefix: Prefix,
    fallback: Option<Offset>,
    raw: Mutex<Raw>,
    ready: Mutex<Ready>,
    progress: Condvar,
    /// How far reading may run ahead of the decoder, in blocks.
    ahead: usize,
    block_bytes: usize,
    max_line: usize,
}

impl BlockFeed {
    /// `ahead` bounds the blocks read and parsed but not yet used.
    #[must_use]
    pub fn new(
        reader: Box<dyn BufRead + Send>,
        prefix: Prefix,
        fallback: Option<Offset>,
        ahead: usize,
    ) -> Self {
        Self::with_sizes(
            reader,
            prefix,
            fallback,
            ahead,
            BLOCK_BYTES,
            super::line::MAX_LINE_BYTES,
        )
    }

    pub(super) fn with_sizes(
        reader: Box<dyn BufRead + Send>,
        prefix: Prefix,
        fallback: Option<Offset>,
        ahead: usize,
        block_bytes: usize,
        max_line: usize,
    ) -> Self {
        BlockFeed {
            prefix,
            fallback,
            raw: Mutex::new(Raw {
                reader: ReaderBox(reader),
                next: 0,
                done: false,
            }),
            ready: Mutex::new(Ready::default()),
            progress: Condvar::new(),
            ahead: ahead.max(1),
            block_bytes: block_bytes.max(1),
            max_line,
        }
    }

    /// Reads and parses the next block if no one else is reading and the decoder is not too far
    /// behind. Returns whether it did.
    pub fn help(&self) -> bool {
        let (index, raw) = {
            let mut raw = match self.raw.try_lock() {
                Ok(raw) => raw,
                Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
                Err(TryLockError::WouldBlock) => return false,
            };
            if raw.done || raw.next >= locked(&self.ready).wanted + self.ahead {
                return false;
            }
            let index = raw.next;
            let block = read_block(&mut *raw.reader.0, self.block_bytes, self.max_line);
            raw.next += 1;
            if block.last {
                raw.done = true;
                locked(&self.ready).total = Some(raw.next);
            }
            (index, block)
        };
        let parsed = parse_block(raw, index == 0, &self.prefix, self.fallback);
        locked(&self.ready).blocks.insert(index, parsed);
        self.progress.notify_all();
        true
    }

    /// The block with number `index`, waiting for it (and reading it, if no one else is). `None`
    /// when the file has no such block.
    pub(super) fn take(&self, index: usize) -> Option<ParsedBlock> {
        loop {
            {
                let mut ready = locked(&self.ready);
                if let Some(block) = ready.blocks.remove(&index) {
                    ready.wanted = index + 1;
                    drop(ready);
                    self.progress.notify_all();
                    return Some(block);
                }
                ready.wanted = index;
                if ready.total.is_some_and(|total| index >= total) {
                    return None;
                }
            }
            if self.help() {
                continue;
            }
            let ready = locked(&self.ready);
            if ready.blocks.contains_key(&index) || ready.total.is_some_and(|total| index >= total)
            {
                continue;
            }
            // Someone is reading or parsing it; the timeout covers a wake-up that was missed.
            drop(
                self.progress
                    .wait_timeout(ready, Duration::from_millis(2))
                    .unwrap_or_else(PoisonError::into_inner),
            );
        }
    }

    /// The prefix lines are parsed with.
    pub(super) fn prefix(&self) -> &Prefix {
        &self.prefix
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{self, BufReader, Read},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
    };

    use super::*;
    use crate::log::{DecodeStats, Decoder, stderr::StderrDecoder};

    const SYNTH: &str = "%m [%p] %q%u@%d/%a %Q ";

    /// A reader that fails after `fail_at` bytes.
    struct FailAfter {
        data: Vec<u8>,
        at: usize,
        fail_at: usize,
    }

    impl Read for FailAfter {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.at >= self.fail_at.min(self.data.len()) {
                return if self.at >= self.data.len() {
                    Ok(0)
                } else {
                    Err(io::Error::other("disk gone"))
                };
            }
            let end = (self.at + buf.len()).min(self.fail_at.min(self.data.len()));
            let n = end - self.at;
            buf[..n].copy_from_slice(&self.data[self.at..end]);
            self.at = end;
            Ok(n)
        }
    }

    /// What decoding gave: the records, whether it ended in an error, and the statistics.
    type Outcome = (Vec<LogRecord>, Option<String>, DecodeStats);

    fn drain(decoder: &mut impl Decoder) -> Outcome {
        let mut records = Vec::new();
        let mut error = None;
        loop {
            match decoder.next_record() {
                Ok(Some(record)) => records.push(record),
                Ok(None) => break,
                Err(e) => {
                    error = Some(e.to_string());
                    break;
                }
            }
        }
        (records, error, decoder.stats().clone())
    }

    fn reader_of(data: &[u8], fail_at: usize) -> BufReader<FailAfter> {
        BufReader::with_capacity(
            13,
            FailAfter {
                data: data.to_vec(),
                at: 0,
                fail_at,
            },
        )
    }

    fn serial(prefix: &str, data: &[u8], fail_at: usize) -> Outcome {
        let mut decoder = StderrDecoder::new(
            reader_of(data, fail_at),
            Prefix::compile(prefix).unwrap(),
            None,
        );
        drain(&mut decoder)
    }

    /// Decodes through blocks of about `block_bytes`, with `helpers` other threads parsing.
    fn blocks(
        prefix: &str,
        data: &[u8],
        fail_at: usize,
        block_bytes: usize,
        helpers: usize,
    ) -> Outcome {
        let feed = Arc::new(BlockFeed::with_sizes(
            Box::new(reader_of(data, fail_at)),
            Prefix::compile(prefix).unwrap(),
            None,
            3,
            block_bytes,
            crate::log::line::MAX_LINE_BYTES,
        ));
        let stop = Arc::new(AtomicBool::new(false));
        let threads: Vec<_> = (0..helpers)
            .map(|_| {
                let (feed, stop) = (Arc::clone(&feed), Arc::clone(&stop));
                thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        if !feed.help() {
                            thread::yield_now();
                        }
                    }
                })
            })
            .collect();
        let mut decoder = StderrDecoder::from_blocks(feed);
        let outcome = drain(&mut decoder);
        stop.store(true, Ordering::Relaxed);
        for thread in threads {
            thread.join().unwrap();
        }
        outcome
    }

    fn same_either_way(prefix: &str, data: &[u8], fail_at: usize) {
        let want = serial(prefix, data, fail_at);
        for block_bytes in [1, 40, 97, 400, 5_000] {
            for helpers in [0, 1, 3] {
                let got = blocks(prefix, data, fail_at, block_bytes, helpers);
                assert_eq!(
                    got, want,
                    "blocks of {block_bytes} bytes, {helpers} helpers, fail at {fail_at}"
                );
            }
        }
    }

    /// A small deterministic generator.
    struct Lcg(u64);

    impl Lcg {
        fn below(&mut self, n: u64) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) % n
        }
    }

    /// A log of lines of every kind the decoder tells apart, in an order that mixes backends.
    fn awkward_log(seed: u64, lines: usize) -> Vec<u8> {
        let mut rng = Lcg(seed);
        let mut out = Vec::new();
        for n in 0..lines {
            let pid = 100 + rng.below(4);
            let head = format!(
                "2026-10-14 10:20:{:02}.{:03} UTC [{pid}] user{}@db/app{} 0",
                n % 60,
                rng.below(1000),
                rng.below(3),
                rng.below(2),
            );
            let end: &[u8] = if rng.below(8) == 0 { b"\r\n" } else { b"\n" };
            match rng.below(14) {
                0..=3 => out.extend(
                    format!(
                        "{head} LOG:  duration: {}.{:03} ms  statement: SELECT {n}",
                        rng.below(50),
                        rng.below(1000)
                    )
                    .bytes(),
                ),
                4 => out.extend(format!("{head} ERROR:  relation \"t{n}\" does not exist").bytes()),
                5 => out.extend(format!("{head} DETAIL:  Key (id)=({n}) already exists.").bytes()),
                6 => out.extend(format!("{head} STATEMENT:  INSERT INTO t VALUES ({n})").bytes()),
                7 => out.extend(format!("\t    continued {n}").bytes()),
                8 => {}
                9 => out.extend(format!("not a log line {n}").bytes()),
                10 => {
                    out.extend(format!("{head} LOG:  caf").bytes());
                    out.extend([0xc3, 0x28, 0xff]);
                }
                11 => out.extend(b" [Your log message was truncated]"),
                12 => out.extend(
                    format!("2026-10-14 10:20:00.000 XYZ [{pid}] u@d/a 0 LOG:  odd zone").bytes(),
                ),
                _ => out.extend(format!("{head} HINT:  try {n}").bytes()),
            }
            // The last line may lack its line break.
            if n + 1 < lines || rng.below(2) == 0 {
                out.extend(end);
            }
        }
        out
    }

    #[test]
    fn blocks_give_what_reading_line_by_line_gives_on_awkward_logs() {
        for seed in 0..25 {
            let data = awkward_log(seed, 60 + usize::try_from(seed).unwrap() * 7);
            same_either_way(SYNTH, &data, usize::MAX);
        }
    }

    #[test]
    fn a_byte_order_mark_a_missing_final_newline_and_blank_lines_agree() {
        let text = "\u{feff}2026-10-14 10:20:12.500 UTC [1] u@d/a 0 LOG:  first\n\n\r\n\
                    2026-10-14 10:20:13.500 UTC [1] u@d/a 0 DETAIL:  more\n\
                    2026-10-14 10:20:14.500 UTC [2] u@d/a 0 LOG:  last";
        same_either_way(SYNTH, text.as_bytes(), usize::MAX);
        same_either_way(SYNTH, b"", usize::MAX);
        same_either_way(SYNTH, b"\n", usize::MAX);
        same_either_way(SYNTH, b"\xef\xbb\xbf", usize::MAX);
    }

    #[test]
    fn a_read_error_comes_after_the_lines_before_it_in_both() {
        let data = awkward_log(7, 80);
        for fail_at in [0, 1, 50, 333, data.len() / 2, data.len() - 1] {
            same_either_way(SYNTH, &data, fail_at);
        }
    }

    #[test]
    fn a_line_over_the_limit_is_skipped_the_same_way() {
        let mut data = b"2026-10-14 10:20:12.500 UTC [1] u@d/a 0 LOG:  before\n".to_vec();
        data.extend(b"2026-10-14 10:20:12.600 UTC [1] u@d/a 0 LOG:  ");
        data.extend(std::iter::repeat_n(
            b'x',
            crate::log::line::MAX_LINE_BYTES + 10,
        ));
        data.extend(b"\n2026-10-14 10:20:12.700 UTC [1] u@d/a 0 LOG:  after\n");
        let want = serial(SYNTH, &data, usize::MAX);
        assert_eq!(want.0.len(), 2);
        for helpers in [0, 2] {
            assert_eq!(blocks(SYNTH, &data, usize::MAX, 1_000, helpers), want);
        }
    }

    #[test]
    fn other_prefixes_agree_too() {
        let rds = "2026-10-14 10:20:09 UTC:10.0.0.1(5432):alice@app:[123]:LOG:  connection authorized\n\
                   2026-10-14 10:20:10 UTC::@:[44]:LOG:  checkpoint starting: time\n\
                   2026-10-14 10:20:11 UTC:10.0.0.1(5432):alice@app:[123]:ERROR:  boom\n\
                   2026-10-14 10:20:11 UTC:10.0.0.1(5432):alice@app:[123]:STATEMENT:  SELECT 1\n";
        same_either_way("%t:%r:%u@%d:[%p]:", rds.as_bytes(), usize::MAX);
        let plain = "2026-10-14 10:20:12.500 UTC LOG:  a\n2026-10-14 10:20:13.500 UTC LOG:  b\n";
        same_either_way("%m ", plain.as_bytes(), usize::MAX);
    }

    #[test]
    fn a_feed_stops_reading_when_the_decoder_is_far_behind() {
        let data = awkward_log(3, 400);
        let feed = BlockFeed::with_sizes(
            Box::new(reader_of(&data, usize::MAX)),
            Prefix::compile(SYNTH).unwrap(),
            None,
            2,
            100,
            crate::log::line::MAX_LINE_BYTES,
        );
        let mut helped = 0;
        while feed.help() {
            helped += 1;
        }
        assert_eq!(
            helped, 2,
            "two blocks ahead of block 0, which nobody has taken"
        );
        assert!(feed.take(0).is_some());
        assert!(feed.help(), "taking a block makes room for another");
    }
}
