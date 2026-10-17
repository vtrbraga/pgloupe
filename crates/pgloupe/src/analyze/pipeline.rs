//! Reading files, decoding them and folding the records into accumulators in parallel.
//!
//! One thread decodes each file in order (stderr entry grouping and csv records need it) and hands
//! batches to rayon workers through `par_bridge`, which pulls a batch only when a worker is free.
//! Each worker folds into its own [`Accumulators`]; the partial results are merged at the end.

use std::{
    collections::VecDeque,
    io::{self, BufRead},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use rayon::prelude::*;
use thiserror::Error;

use super::{
    Merge,
    acc::Accumulators,
    anonymize::{self, Anonymizer},
    correlate::Correlator,
    filter::Filter,
    slice::Slices,
};
use crate::log::{
    BlockPair, DecodeError, DecodeStats, Decoder, Format, LogRecord, Options as LogOptions,
    SetupError, Severity, decoder_for,
    infer::{Inference, SAMPLE_LINES, infer_prefix},
    locale::{self, Translator},
    prefix::Prefix,
    source,
    tee::{self, Content, TeeHandle},
};

/// A worker keeps up to this many times the requested fingerprints before the final re-cap, so the
/// result does not depend on how records were split across workers.
const WORKER_LIMIT_FACTOR: usize = 4;

/// Settings of an analysis run.
#[derive(Debug, Clone)]
pub struct Options {
    /// Force a log format; `None` detects it per file.
    pub format: Option<Format>,
    pub prefix: Option<String>,
    pub log_tz: Option<jiff::tz::Offset>,
    pub max_fingerprints: usize,
    pub bucket_secs: i64,
    /// Worker threads; `None` uses all cores.
    pub threads: Option<usize>,
    /// Records handed to a worker at a time.
    pub batch_size: usize,
    /// When set, the decompressed bytes of each file are hashed while they are read, and the
    /// running hash is snapshotted at these lengths (see [`Content`]).
    pub hash_marks: Option<Vec<u64>>,
    /// Slowest single executions to keep.
    pub slowest: usize,
    /// Keep bind parameters with the slowest executions.
    pub params: bool,
    /// Which records take part.
    pub filter: Filter,
    /// The language of the server's messages: `None` detects it per file, `Some("en")` reads them
    /// as English, anything else names a language of the built-in table or of `messages`.
    pub locale: Option<String>,
    /// An extra translation table (see `log::locale`), as text.
    pub messages: Option<String>,
}

impl Options {
    /// Defaults: detect formats, 50,000 fingerprints, one-minute buckets, 4096-record batches.
    #[must_use]
    pub fn new() -> Self {
        Options {
            format: None,
            prefix: None,
            log_tz: None,
            max_fingerprints: 50_000,
            bucket_secs: 60,
            threads: None,
            batch_size: 4_096,
            hash_marks: None,
            slowest: super::slowest::DEFAULT_KEPT,
            params: false,
            filter: Filter::default(),
            locale: None,
            messages: None,
        }
    }
}

impl Default for Options {
    fn default() -> Self {
        Self::new()
    }
}

/// Where a stderr file's prefix came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefixSource {
    Given,
    Inferred { matched: usize, sampled: usize },
    NotApplicable,
}

/// What happened to one input file.
#[derive(Debug, Clone)]
pub struct FileSummary {
    pub path: PathBuf,
    pub bytes: u64,
    pub format: Format,
    pub prefix: Option<String>,
    pub prefix_source: PrefixSource,
    pub stats: DecodeStats,
    /// Records the filter dropped.
    pub filtered: u64,
    /// Identity of the decompressed bytes; only when `Options::hash_marks` is set.
    pub content: Option<Content>,
    /// The language the messages were translated from, when it was not English.
    pub locale: Option<String>,
}

/// The result of a run.
#[derive(Debug)]
pub struct Output {
    pub acc: Accumulators,
    pub files: Vec<FileSummary>,
}

/// Why an analysis could not complete.
#[derive(Debug, Error)]
pub enum AnalyzeError {
    #[error("{}: could not open", .path.display())]
    Open {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{}: could not read", .path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: DecodeError,
    },
    #[error("{}: cannot decode", .path.display())]
    Setup {
        path: PathBuf,
        #[source]
        source: SetupError,
    },
    #[error("{}: looks like binary data, not a log; pass --format to force a decoder", .path.display())]
    Binary { path: PathBuf },
    #[error("{}: could not infer the log_line_prefix; pass --prefix with the server's setting", .path.display())]
    PrefixNotInferred { path: PathBuf, inference: Inference },
    #[error("{}: {message}", .path.display())]
    Locale { path: PathBuf, message: String },
    #[error("could not start the worker pool")]
    Pool(#[source] rayon::ThreadPoolBuildError),
}

/// Reads `paths` in order and returns the merged statistics.
///
/// # Errors
///
/// Fails when a file cannot be opened or read, is binary, is a stderr log whose prefix cannot be
/// inferred, or the worker pool cannot start.
pub fn run(paths: &[PathBuf], options: &Options) -> Result<Output, AnalyzeError> {
    let (mut acc, files) = run_with(paths, options, || {
        Accumulators::new(worker_limit(options), options.bucket_secs)
            .with_capture(options.slowest, options.params)
    })?;
    acc.recap(options.max_fingerprints);
    Ok(Output { acc, files })
}

/// One file folded into per-hour slices (one-minute timeline buckets, the store's resolution).
#[derive(Debug)]
pub struct SlicedOutput {
    pub slices: Slices,
    pub file: FileSummary,
}

/// Reads one file into per-hour slices, each capped at `options.max_fingerprints`.
///
/// # Errors
///
/// Fails like [`run`].
pub fn run_sliced(path: &Path, options: &Options) -> Result<SlicedOutput, AnalyzeError> {
    let paths = [path.to_path_buf()];
    let (mut slices, mut files) = run_with(&paths, options, || {
        Slices::new(worker_limit(options), 60).with_capture(options.slowest, options.params)
    })?;
    slices.recap(options.max_fingerprints);
    let file = files.pop().ok_or_else(|| AnalyzeError::Open {
        path: path.to_path_buf(),
        source: io::Error::other("no file summary"),
    })?;
    Ok(SlicedOutput { slices, file })
}

fn worker_limit(options: &Options) -> usize {
    options
        .max_fingerprints
        .saturating_mul(WORKER_LIMIT_FACTOR)
        .max(1)
}

/// What a worker folds records into.
trait Fold: Merge + Send {
    fn add(&mut self, record: &LogRecord);
    fn add_blocks(&mut self, blocks: &[BlockPair]);
}

impl Fold for Accumulators {
    fn add(&mut self, record: &LogRecord) {
        self.add_record(record);
    }

    fn add_blocks(&mut self, blocks: &[BlockPair]) {
        Self::add_blocks(self, blocks);
    }
}

impl Fold for Slices {
    fn add(&mut self, record: &LogRecord) {
        self.add_record(record);
    }

    fn add_blocks(&mut self, blocks: &[BlockPair]) {
        Self::add_blocks(self, blocks);
    }
}

fn run_with<S: Fold>(
    paths: &[PathBuf],
    options: &Options,
    new: impl Fn() -> S + Sync + Send,
) -> Result<(S, Vec<FileSummary>), AnalyzeError> {
    let summaries = Arc::new(Mutex::new(Vec::new()));
    let failure = Arc::new(Mutex::new(None));
    let batches = Batches {
        correlator: Correlator::new(),
        filtering: options.filter.narrows(),
        anonymizer: options.filter.anonymize.as_deref().map(Anonymizer::new),
        pending: paths.iter().cloned().collect(),
        current: None,
        options: options.clone(),
        summaries: Arc::clone(&summaries),
        failure: Arc::clone(&failure),
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(options.threads.unwrap_or(0))
        .build()
        .map_err(AnalyzeError::Pool)?;
    let acc = pool.install(|| {
        batches
            .par_bridge()
            .fold(&new, |mut acc, batch| {
                for record in &batch.records {
                    acc.add(record);
                }
                acc.add_blocks(&batch.blocks);
                acc
            })
            .reduce(&new, |mut left, right| {
                left.merge(right);
                left
            })
    });
    if let Some(error) = lock(&failure).take() {
        return Err(error);
    }
    let mut files = std::mem::take(&mut *lock(&summaries));
    files.sort_by_key(|file: &(usize, FileSummary)| file.0);
    Ok((acc, files.into_iter().map(|(_, file)| file).collect()))
}

/// Locks a mutex, recovering the data if another thread panicked while holding it.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The file being decoded.
struct Current {
    index: usize,
    path: PathBuf,
    bytes: u64,
    format: Format,
    prefix: Option<String>,
    prefix_source: PrefixSource,
    decoder: Box<dyn Decoder + Send>,
    tee: Option<TeeHandle>,
    /// Records the filter dropped so far.
    filtered: u64,
    /// Rewrites another language's messages as English.
    translator: Option<(String, Translator)>,
}

/// Records for the workers, and the lock waits the correlator settled while reading them. The
/// waits travel beside the records: the record that settles one may be filtered out.
struct Batch {
    records: Vec<LogRecord>,
    blocks: Vec<BlockPair>,
}

/// Yields batches of records, file after file.
struct Batches {
    correlator: Correlator,
    /// Whether the filter drops anything; if not, no record is asked.
    filtering: bool,
    anonymizer: Option<Anonymizer>,
    pending: VecDeque<PathBuf>,
    current: Option<Current>,
    options: Options,
    summaries: Arc<Mutex<Vec<(usize, FileSummary)>>>,
    failure: Arc<Mutex<Option<AnalyzeError>>>,
}

impl Batches {
    fn opened(&self) -> usize {
        lock(&self.summaries).len() + usize::from(self.current.is_some())
    }

    fn finish_current(&mut self) {
        if let Some(current) = self.current.take() {
            let mut stats = current.decoder.stats().clone();
            if self.options.filter.anonymize.is_some() {
                stats.redact();
            }
            lock(&self.summaries).push((
                current.index,
                FileSummary {
                    path: current.path,
                    bytes: current.bytes,
                    format: current.format,
                    prefix: current.prefix,
                    prefix_source: current.prefix_source,
                    stats,
                    filtered: current.filtered,
                    content: current.tee.as_ref().map(TeeHandle::finish),
                    locale: current.translator.as_ref().map(|(name, _)| name.clone()),
                },
            ));
        }
    }

    fn fail(&mut self, error: AnalyzeError) {
        *lock(&self.failure) = Some(error);
        self.finish_current();
        self.pending.clear();
    }

    fn open_next(&mut self) -> Option<()> {
        let path = self.pending.pop_front()?;
        // Backend process ids mean nothing across files: each starts with a clean slate.
        self.correlator = Correlator::new();
        let index = self.opened();
        match open_file(&path, index, &self.options) {
            Ok(current) => self.current = Some(current),
            Err(error) => self.fail(error),
        }
        Some(())
    }
}

impl Iterator for Batches {
    type Item = Batch;

    fn next(&mut self) -> Option<Batch> {
        loop {
            if self.current.is_none() {
                self.open_next()?;
                continue;
            }
            let mut batch = Vec::new();
            let mut blocks = Vec::new();
            let mut error = None;
            let mut finished = false;
            if let Some(current) = self.current.as_mut() {
                while batch.len() < self.options.batch_size.max(1) {
                    match current.decoder.next_record() {
                        Ok(Some(mut record)) => {
                            if let Some((name, translator)) = &current.translator {
                                translate(translator, &mut record);
                                if record.level.is_error() && record.sqlstate.is_some() {
                                    record.locale = Some(name.clone());
                                }
                            }
                            // Before the filter: a cursor declared by a record the filter drops
                            // still has to be known when its fetches arrive.
                            self.correlator.apply(&mut record);
                            blocks.append(&mut record.blocks);
                            if record.correlation.as_ref().is_some_and(|c| c.discard) {
                                current.filtered += 1;
                                continue;
                            }
                            let keep = !self.filtering || self.options.filter.keeps(&record);
                            if !keep {
                                self.correlator.dropped(&record);
                            }
                            if keep {
                                if let Some(anonymizer) = &mut self.anonymizer {
                                    anonymizer.apply(&mut record);
                                }
                                batch.push(record);
                            } else {
                                current.filtered += 1;
                            }
                        }
                        Ok(None) => {
                            finished = true;
                            break;
                        }
                        Err(source) => {
                            error = Some(AnalyzeError::Read {
                                path: current.path.clone(),
                                source,
                            });
                            break;
                        }
                    }
                }
            }
            if finished {
                blocks.extend(self.correlator.finish());
            }
            if self.anonymizer.is_some() {
                blocks.iter_mut().for_each(anonymize::scrub_pair);
            }
            if let Some(error) = error {
                self.fail(error);
            } else if finished {
                self.finish_current();
            }
            if !batch.is_empty() || !blocks.is_empty() {
                return Some(Batch {
                    records: batch,
                    blocks,
                });
            }
            if self.current.is_none() && self.pending.is_empty() {
                return None;
            }
        }
    }
}

fn open_file(path: &Path, index: usize, options: &Options) -> Result<Current, AnalyzeError> {
    let open_err = |source| AnalyzeError::Open {
        path: path.to_path_buf(),
        source,
    };
    let bytes = std::fs::metadata(path).map_err(open_err)?.len();
    let reader = source::open(path).map_err(open_err)?;
    let (mut reader, tee) = match &options.hash_marks {
        Some(marks) => {
            let (tee, handle) = tee::tee(reader, marks.clone());
            (Box::new(tee) as Box<dyn BufRead + Send>, Some(handle))
        }
        None => (reader, None),
    };
    let peek = reader.fill_buf().map_err(open_err)?;
    let blank = peek.iter().all(u8::is_ascii_whitespace);
    let format = match options.format {
        Some(format) => format,
        None if peek.iter().all(u8::is_ascii_whitespace) => Format::Json,
        None if peek.contains(&0) => {
            return Err(AnalyzeError::Binary {
                path: path.to_path_buf(),
            });
        }
        None => match (source::sniff(peek), options.prefix.as_deref()) {
            // A stderr prefix that starts like a csv row (`%m,`) looks like csv; the prefix wins
            // only when it really matches the first line.
            (Format::Csv, Some(prefix)) if prefix_fits(prefix, peek, options.log_tz) => {
                Format::Stderr
            }
            (sniffed, _) => sniffed,
        },
    };
    let translator =
        translator_for(&String::from_utf8_lossy(peek), options).map_err(|message| {
            AnalyzeError::Locale {
                path: path.to_path_buf(),
                message,
            }
        })?;
    let (prefix, prefix_source) = if format == Format::Stderr && !blank {
        match &options.prefix {
            Some(prefix) => (Some(prefix.clone()), PrefixSource::Given),
            None => infer(path, options)?,
        }
    } else {
        (None, PrefixSource::NotApplicable)
    };
    let log_options = LogOptions {
        prefix: prefix.clone(),
        log_tz: options.log_tz,
    };
    // A blank-only stderr file has no prefix to decode with; any decoder reads it as empty.
    let decode_format = if format == Format::Stderr && prefix.is_none() {
        Format::Json
    } else {
        format
    };
    let decoder =
        decoder_for(decode_format, reader, &log_options).map_err(|source| AnalyzeError::Setup {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(Current {
        index,
        path: path.to_path_buf(),
        bytes,
        format,
        prefix,
        prefix_source,
        decoder,
        tee,
        filtered: 0,
        translator,
    })
}

/// The translator for a file, from the options or from what its first lines look like.
fn translator_for(sample: &str, options: &Options) -> Result<Option<(String, Translator)>, String> {
    let name = match options.locale.as_deref() {
        Some("en" | "C") => return Ok(None),
        Some(name) => name.to_owned(),
        None => match locale::detect(sample) {
            Some(found) => found.to_owned(),
            None => return Ok(None),
        },
    };
    let translator = Translator::new(&name, options.messages.as_deref())?;
    if translator.is_empty() {
        return Err(format!(
            "no translations for {name:?}; the built-in languages are {}; add one with --messages-file",
            locale::languages().join(", ")
        ));
    }
    Ok(Some((name, translator)))
}

/// Rewrites the message and the DETAIL of `record` as English when a rule recognizes them.
fn translate(translator: &Translator, record: &mut LogRecord) {
    if record.cursor_position.is_none()
        && !matches!(record.level, Severity::Log | Severity::Debug)
        && let Some((message, position)) = translator.split_position(&record.message)
    {
        record.cursor_position = Some(position);
        record.message = message.to_owned();
    }
    if let Some(english) = translator.apply(&record.message) {
        record.message = english;
    }
    if let Some(detail) = record.detail.as_deref().and_then(|d| translator.apply(d)) {
        record.detail = Some(detail);
    }
}

/// True if `prefix` matches the first non-blank line in `peek`.
fn prefix_fits(prefix: &str, peek: &[u8], fallback: Option<jiff::tz::Offset>) -> bool {
    let Ok(prefix) = Prefix::compile(prefix) else {
        return false;
    };
    let text = String::from_utf8_lossy(peek);
    let Some(line) = text.lines().find(|line| !line.trim().is_empty()) else {
        return false;
    };
    let mut record = LogRecord::new(jiff::Timestamp::UNIX_EPOCH, Severity::Log, String::new());
    prefix.parse_line(line, &mut record, fallback).is_ok()
}

/// Infers the prefix of a stderr file by reading its first lines through a second reader.
fn infer(path: &Path, options: &Options) -> Result<(Option<String>, PrefixSource), AnalyzeError> {
    let open_err = |source| AnalyzeError::Open {
        path: path.to_path_buf(),
        source,
    };
    let mut reader = source::open(path).map_err(open_err)?;
    let mut lines = Vec::new();
    let mut buf = Vec::new();
    while lines.len() < SAMPLE_LINES * 2 {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        // A line that is not valid UTF-8 must not end (or fail) the sample.
        lines.push(
            String::from_utf8_lossy(&buf)
                .trim_end_matches(['\n', '\r'])
                .to_owned(),
        );
    }
    let inference = infer_prefix(&lines, options.log_tz);
    match inference.prefix.clone() {
        Some(prefix) => {
            let matched = inference.best.as_ref().map_or(0, |(_, matched)| *matched);
            Ok((
                Some(prefix),
                PrefixSource::Inferred {
                    matched,
                    sampled: inference.sampled,
                },
            ))
        }
        None => Err(AnalyzeError::PrefixNotInferred {
            path: path.to_path_buf(),
            inference,
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn slices_merge_to_what_run_computes() {
        let path = fixture("sample.json");
        let whole = run(std::slice::from_ref(&path), &Options::new()).unwrap();
        let sliced = run_sliced(&path, &Options::new()).unwrap();
        let mut merged = Accumulators::new(Options::new().max_fingerprints, 60);
        for (_, acc) in sliced.slices.by_hour {
            merged.merge(acc);
        }
        merged.recap(Options::new().max_fingerprints);
        assert_eq!(merged.totals, whole.acc.totals);
        assert_eq!(merged.queries, whole.acc.queries);
        assert_eq!(merged.overflow, whole.acc.overflow);
    }

    #[test]
    fn slices_are_keyed_by_utc_hour() {
        let log = [
            r#"{"timestamp":"2026-10-14 10:59:59.000 UTC","pid":1,"session_id":"a.1","line_num":1,"error_severity":"LOG","message":"duration: 1.0 ms  statement: SELECT 1","backend_type":"client backend"}"#,
            r#"{"timestamp":"2026-10-14 11:00:00.000 UTC","pid":1,"session_id":"a.1","line_num":2,"error_severity":"LOG","message":"duration: 2.0 ms  statement: SELECT 1","backend_type":"client backend"}"#,
        ]
        .join("\n");
        let path = scratch("hours.json", log.as_bytes());
        let sliced = run_sliced(&path, &Options::new()).unwrap();
        let hours: Vec<i64> = sliced.slices.by_hour.keys().copied().collect();
        assert_eq!(hours.len(), 2);
        assert_eq!(hours[1] - hours[0], 3_600);
        assert_eq!(hours[0] % 3_600, 0);
        for acc in sliced.slices.by_hour.values() {
            assert_eq!(acc.totals.records, 1);
        }
    }

    #[test]
    fn slices_do_not_depend_on_threads_or_batches() {
        let path = repeated("sample.json", 40);
        let slices = |threads, batch_size| {
            run_sliced(
                &path,
                &Options {
                    threads: Some(threads),
                    batch_size,
                    ..Options::new()
                },
            )
            .unwrap()
            .slices
        };
        assert_eq!(slices(1, 4_096), slices(4, 3));
    }

    #[test]
    fn content_covers_every_byte_in_every_format() {
        for (name, prefix) in [
            ("sample.csv", None),
            ("sample.json", None),
            ("sample.log", Some("%m [%p] %q%u@%d/%a %Q ")),
        ] {
            let path = fixture(name);
            let bytes = fs::read(&path).unwrap();
            let options = Options {
                hash_marks: Some(vec![10]),
                prefix: prefix.map(str::to_owned),
                ..Options::new()
            };
            let out = run_sliced(&path, &options).unwrap();
            let content = out
                .file
                .content
                .unwrap_or_else(|| panic!("{name}: no content"));
            assert_eq!(content.size, bytes.len() as u64, "{name}");
            assert_eq!(content.hash, xxhash_rust::xxh3::xxh3_128(&bytes), "{name}");
            assert_eq!(
                content.prefixes,
                vec![(10, xxhash_rust::xxh3::xxh3_128(&bytes[..10]))],
                "{name}"
            );
        }
    }

    #[test]
    fn no_content_is_computed_unless_asked() {
        let out = run_sliced(&fixture("sample.json"), &Options::new()).unwrap();
        assert!(out.file.content.is_none());
    }

    #[test]
    fn accumulators_survive_json() {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/analyze/events.json");
        let output = run(&[path], &Options::new()).unwrap();
        let json = serde_json::to_string(&output.acc).unwrap();
        let back: Accumulators = serde_json::from_str(&json).unwrap();
        assert_eq!(back, output.acc);
    }

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/logs")
            .join(name)
    }

    fn scratch(name: &str, contents: &[u8]) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("pgloupe-pipeline-{}-{name}", std::process::id()));
        fs::write(&path, contents).unwrap();
        path
    }

    fn repeated(name: &str, times: usize) -> PathBuf {
        let one = fs::read(fixture(name)).unwrap();
        let mut bytes = Vec::new();
        for _ in 0..times {
            bytes.extend_from_slice(&one);
            if !one.ends_with(b"\n") {
                bytes.push(b'\n');
            }
        }
        scratch(&format!("{times}x{name}"), &bytes)
    }

    fn options() -> Options {
        Options {
            prefix: Some("%m [%p] %q%u@%d/%a %Q ".to_owned()),
            ..Options::new()
        }
    }

    #[test]
    fn analyzes_each_format_of_the_sample() {
        for name in ["sample.csv", "sample.json", "sample.log"] {
            let output = run(&[fixture(name)], &options()).unwrap();
            assert_eq!(output.acc.totals.records, 8, "{name}");
            assert_eq!(output.files.len(), 1);
            assert_eq!(output.files[0].stats.skipped, 0, "{name}");
            assert_eq!(output.acc.totals.events["statement"], 2, "{name}");
            assert_eq!(output.acc.totals.events["temp_file"], 1, "{name}");
        }
    }

    #[test]
    fn formats_are_detected_and_prefixes_reported() {
        let csv = run(&[fixture("sample.csv")], &Options::new()).unwrap();
        assert_eq!(csv.files[0].format, Format::Csv);
        assert_eq!(csv.files[0].prefix_source, PrefixSource::NotApplicable);
        let given = run(&[fixture("sample.log")], &options()).unwrap();
        assert_eq!(given.files[0].prefix_source, PrefixSource::Given);
        let inferred = run(&[fixture("sample.log")], &Options::new()).unwrap();
        assert!(matches!(
            inferred.files[0].prefix_source,
            PrefixSource::Inferred { .. }
        ));
        assert_eq!(
            inferred.files[0].prefix.as_deref(),
            Some("%m [%p] %q%u@%d/%a %Q ")
        );
        assert_eq!(inferred.acc.totals.records, 8);
    }

    #[test]
    fn the_result_does_not_depend_on_threads_or_batch_size() {
        let path = repeated("sample.json", 200);
        let reference = run(
            std::slice::from_ref(&path),
            &Options {
                threads: Some(1),
                batch_size: 4_096,
                ..Options::new()
            },
        )
        .unwrap();
        assert_eq!(reference.acc.totals.records, 1_600);
        for (threads, batch) in [(1, 1), (4, 7), (4, 4_096), (8, 100), (3, 1_599)] {
            let other = run(
                std::slice::from_ref(&path),
                &Options {
                    threads: Some(threads),
                    batch_size: batch,
                    ..Options::new()
                },
            )
            .unwrap();
            assert_eq!(other.acc, reference.acc, "threads {threads}, batch {batch}");
        }
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn empty_and_overlapping_files_merge() {
        let empty = scratch("empty.json", b"");
        let a = fixture("sample.json");
        let b = fixture("sample.json");
        let output = run(&[a, empty.clone(), b], &Options::new()).unwrap();
        assert_eq!(output.files.len(), 3);
        assert_eq!(output.acc.totals.records, 16);
        assert_eq!(output.files[1].stats.records, 0);
        fs::remove_file(empty).unwrap();
    }

    #[test]
    fn a_missing_file_is_an_error_naming_it() {
        let err = run(&[PathBuf::from("/nonexistent/x.log")], &Options::new()).unwrap_err();
        assert!(matches!(err, AnalyzeError::Open { .. }), "{err}");
        assert!(err.to_string().contains("/nonexistent/x.log"), "{err}");
    }

    #[test]
    fn an_unknown_prefix_fails_with_the_inference_report() {
        let path = scratch(
            "odd.log",
            b"<2026-10-14T10:20:09> pid=42 LOG:  hello\n<2026-10-14T10:20:10> pid=42 LOG:  again\n",
        );
        let err = run(std::slice::from_ref(&path), &Options::new()).unwrap_err();
        fs::remove_file(path).unwrap();
        let AnalyzeError::PrefixNotInferred { inference, .. } = err else {
            panic!("expected a prefix error, got {err}");
        };
        assert_eq!(inference.prefix, None);
        assert_ne!(inference.unmatched, Vec::<String>::new());
    }

    #[test]
    fn binary_input_is_refused() {
        let path = scratch("bin.bin", &[0, 1, 2, 3, 0]);
        let err = run(std::slice::from_ref(&path), &Options::new()).unwrap_err();
        fs::remove_file(path).unwrap();
        assert!(matches!(err, AnalyzeError::Binary { .. }), "{err}");
    }

    #[test]
    fn the_fingerprint_cap_applies_to_the_final_result() {
        let path = repeated("sample.json", 3);
        let output = run(
            std::slice::from_ref(&path),
            &Options {
                max_fingerprints: 1,
                ..Options::new()
            },
        )
        .unwrap();
        fs::remove_file(path).unwrap();
        assert_eq!(output.acc.queries.len(), 1);
        assert!(output.acc.overflow.calls > 0);
    }

    #[test]
    fn a_non_utf8_first_line_does_not_defeat_prefix_inference() {
        let mut bytes = b"2026-10-14 10:20:09.302 UTC [42] LOG:  caf\xE9\n".to_vec();
        for i in 0..50 {
            bytes.extend_from_slice(
                format!("2026-10-14 10:20:10.{i:03} UTC [42] LOG:  fine\n").as_bytes(),
            );
        }
        let path = scratch("latin1.log", &bytes);
        let output = run(std::slice::from_ref(&path), &Options::new()).unwrap();
        fs::remove_file(path).unwrap();
        assert_eq!(output.files[0].prefix.as_deref(), Some("%m [%p] "));
        assert_eq!(output.acc.totals.records, 50);
        assert_eq!(output.files[0].stats.skipped, 1);
    }

    #[test]
    fn a_blank_only_file_is_fine_even_with_an_explicit_stderr_format() {
        let path = scratch("blank.log", b"\n  \n\n");
        let options = Options {
            format: Some(Format::Stderr),
            ..Options::new()
        };
        let output = run(std::slice::from_ref(&path), &options).unwrap();
        fs::remove_file(path).unwrap();
        assert_eq!(output.acc.totals.records, 0);
    }
}
