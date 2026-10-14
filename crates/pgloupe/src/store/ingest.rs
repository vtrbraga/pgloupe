//! Putting log files into the store: unchanged, replaced (grown) or new.

use std::{
    error::Error,
    fs, io,
    path::{Path, PathBuf},
};

use thiserror::Error;

use super::{Input, Store, StoreError, StoredDiag};
use crate::{
    analyze::{
        diagnostics::{file_warnings, top_unmatched_shapes},
        pipeline::{self, AnalyzeError, FileSummary, SlicedOutput},
        report::file_report,
        slice::Slices,
    },
    log::{Format, tee::Content},
};

/// What happened to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// New: its statistics were added.
    Ingested,
    /// Already in the store (same bytes), nothing changed.
    Unchanged,
    /// It extends or repeats earlier input(s); that many were replaced by this one.
    Replaced(usize),
    /// Nothing was stored for it.
    Failed(String),
}

/// The result for one path given to [`ingest`].
#[derive(Debug, Clone)]
pub struct FileOutcome {
    pub path: PathBuf,
    pub status: Status,
    /// Records decoded (or, for `Unchanged`, recorded earlier).
    pub records: u64,
    /// Lines that could not be decoded (or, for `Unchanged`, recorded earlier).
    pub skipped: u64,
    /// What the run noticed about the file; only for files that were read.
    pub diag: Option<StoredDiag>,
}

/// Why one file could not be ingested.
#[derive(Debug, Error)]
enum IngestError {
    #[error("could not read: {0}")]
    Unreadable(#[source] io::Error),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Analyze(#[from] AnalyzeError),
    #[error(
        "nothing could be decoded: all {skipped} non-blank lines were skipped — check --format and --prefix (and lc_messages for non-English servers)"
    )]
    NothingDecoded { skipped: u64 },
    #[error("internal error: no content hash")]
    NoContentHash,
}

/// `error` and its causes, joined with `: `.
fn chain(error: &dyn Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

/// Adds each of `paths` to `store`.
///
/// A file whose decompressed bytes are already stored is left alone (unless `force`); one that
/// extends an earlier input replaces it; anything else is new. A file that cannot be ingested is
/// reported and does not stop the others.
pub fn ingest(
    store: &mut Store,
    paths: &[PathBuf],
    options: &pipeline::Options,
    force: bool,
) -> Vec<FileOutcome> {
    paths
        .iter()
        .map(|path| {
            ingest_one(store, path, options, force).unwrap_or_else(|error| FileOutcome {
                path: path.clone(),
                status: Status::Failed(chain(&error)),
                records: 0,
                skipped: 0,
                diag: None,
            })
        })
        .collect()
}

/// How a file looks on disk, for the "unchanged" shortcut.
struct Stat {
    /// Canonical path, so the same file under two spellings is one.
    key: String,
    disk_size: u64,
    mtime: i64,
}

fn stat_of(path: &Path) -> Result<Stat, IngestError> {
    let meta = fs::metadata(path).map_err(IngestError::Unreadable)?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|time| jiff::Timestamp::try_from(time).ok())
        .map_or(0, jiff::Timestamp::as_second);
    let key = fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string();
    Ok(Stat {
        key,
        disk_size: meta.len(),
        mtime,
    })
}

/// What to do with a file that has been read.
enum Decision {
    /// Its bytes are already stored as this input.
    Known { input: Box<Input> },
    /// Store it, deleting these inputs.
    Store { replace: Vec<i64> },
}

fn decide(
    store: &mut Store,
    content: &Content,
    stat: &Stat,
    force: bool,
) -> Result<Decision, StoreError> {
    if let Some(known) = store.input_by_hash(content.hash)? {
        if force {
            return Ok(Decision::Store {
                replace: vec![known.id],
            });
        }
        store.touch(known.id, &stat.key, stat.disk_size, stat.mtime)?;
        return Ok(Decision::Known {
            input: Box::new(known),
        });
    }
    // An earlier input that is a byte prefix of this file: this file is its later version.
    let mut replace = Vec::new();
    for (size, hash) in &content.prefixes {
        if *size < content.size
            && let Some(id) = store.input_with_prefix(*size, *hash)?
            && !replace.contains(&id)
        {
            replace.push(id);
        }
    }
    Ok(Decision::Store { replace })
}

fn ingest_one(
    store: &mut Store,
    path: &Path,
    options: &pipeline::Options,
    force: bool,
) -> Result<FileOutcome, IngestError> {
    // One writer at a time: what is looked up below must still hold when the file is committed.
    store.begin_ingest()?;
    let outcome = ingest_locked(store, path, options, force);
    let released = store.end_ingest();
    let outcome = outcome?;
    released?;
    Ok(outcome)
}

fn ingest_locked(
    store: &mut Store,
    path: &Path,
    options: &pipeline::Options,
    force: bool,
) -> Result<FileOutcome, IngestError> {
    let stat = stat_of(path)?;
    let unchanged = |known: &Input| FileOutcome {
        path: path.to_path_buf(),
        status: Status::Unchanged,
        records: known.records,
        skipped: known.skipped,
        diag: settings_hint(known, options),
    };
    if !force && let Some(known) = store.input_by_stat(&stat.key, stat.disk_size, stat.mtime)? {
        return Ok(unchanged(&known));
    }
    let run_options = pipeline::Options {
        hash_marks: Some(store.input_sizes()?),
        ..options.clone()
    };
    let SlicedOutput { slices, file } = pipeline::run_sliced(path, &run_options)?;
    let content = file.content.clone().ok_or(IngestError::NoContentHash)?;
    if file.stats.records == 0 && file.stats.skipped > 0 {
        return Err(IngestError::NothingDecoded {
            skipped: file.stats.skipped,
        });
    }
    let replace = match decide(store, &content, &stat, force)? {
        Decision::Known { input } => return Ok(unchanged(&input)),
        Decision::Store { replace } => replace,
    };
    let (mut input, mut diag) = make_input(&file, &slices, &content, stat);
    if let Some(warning) = overlap_warning(store, &input, &content.prefixes)? {
        input.diag.warnings.push(warning.clone());
        diag.warnings.push(warning);
    }
    store.commit(&input, &slices.by_hour, &replace)?;
    Ok(FileOutcome {
        path: path.to_path_buf(),
        status: if replace.is_empty() {
            Status::Ingested
        } else {
            Status::Replaced(replace.len())
        },
        records: file.stats.records,
        skipped: file.stats.skipped,
        diag: Some(diag),
    })
}

/// What to tell someone who gave `--prefix` or `--format` for a file that is already stored with
/// different ones: the stored numbers stay until they ask for a re-ingest.
fn settings_hint(known: &Input, options: &pipeline::Options) -> Option<StoredDiag> {
    let mut warnings = Vec::new();
    if let Some(given) = &options.prefix
        && known.summary.prefix.as_deref() != Some(given.as_str())
    {
        warnings.push(format!(
            "already stored with prefix {:?}, not the one you gave — use --force to ingest it again",
            known.summary.prefix.as_deref().unwrap_or("none")
        ));
    }
    if let Some(format) = options.format {
        let name = match format {
            Format::Csv => "csv",
            Format::Json => "json",
            Format::Stderr => "stderr",
        };
        if known.format != name {
            warnings.push(format!(
                "already stored as {}, not {name} — use --force to ingest it again",
                known.format
            ));
        }
    }
    (!warnings.is_empty()).then_some(StoredDiag {
        lines: 0,
        warnings,
        shapes: Vec::new(),
    })
}

/// Notes an earlier input whose time range largely overlaps `input`'s. A file that extends an
/// earlier one is recognised exactly and replaces it; a smaller copy of a stored file, or two
/// different continuations of one, cannot be, and their events would be counted twice.
fn overlap_warning(
    store: &Store,
    input: &Input,
    prefixes: &[(u64, u128)],
) -> Result<Option<String>, StoreError> {
    let (Some(first), Some(last)) = (input.first_ts, input.last_ts) else {
        return Ok(None);
    };
    for other in store.inputs_in(i64::MIN, i64::MAX)? {
        let replaced = other.hash == input.hash
            || prefixes
                .iter()
                .any(|(size, hash)| *size == other.size && *hash == other.hash);
        let (false, Some(other_first), Some(other_last)) =
            (replaced, other.first_ts, other.last_ts)
        else {
            continue;
        };
        let shorter = (last - first).min(other_last - other_first);
        let overlap = last.min(other_last) - first.max(other_first);
        if shorter > 0 && overlap * 2 >= shorter {
            return Ok(Some(format!(
                "its time range overlaps {} by {}% of the shorter one — if one is a partial copy of the other, their events are counted twice; keep only the complete file or recreate the store",
                other.path,
                overlap * 100 / shorter
            )));
        }
    }
    Ok(None)
}

fn make_input(
    file: &FileSummary,
    slices: &Slices,
    content: &Content,
    stat: Stat,
) -> (Input, StoredDiag) {
    let first_ts = slices
        .by_hour
        .values()
        .filter_map(|acc| acc.totals.first)
        .min()
        .map(jiff::Timestamp::as_second);
    let last_ts = slices
        .by_hour
        .values()
        .filter_map(|acc| acc.totals.last)
        .max()
        .map(jiff::Timestamp::as_second);
    let one = std::slice::from_ref(file);
    let mut warnings = file_warnings(one);
    if content.size == 0 {
        warnings.push("file is empty".to_owned());
    }
    let diag = StoredDiag {
        lines: file.stats.lines,
        warnings,
        shapes: top_unmatched_shapes(one),
    };
    let mut summary = file_report(file);
    summary.path.clone_from(&stat.key);
    let input = Input {
        id: 0,
        hash: content.hash,
        size: content.size,
        path: stat.key,
        disk_size: stat.disk_size,
        mtime: stat.mtime,
        format: summary.format.clone(),
        records: file.stats.records,
        skipped: file.stats.skipped,
        first_ts,
        last_ts,
        summary,
        diag: diag.clone(),
        ingested_at: jiff::Timestamp::now().as_second(),
    };
    (input, diag)
}

#[cfg(test)]
mod tests {
    use std::{
        io::Write,
        time::{Duration, SystemTime},
    };

    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pgloupe-ingest-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn store_in(dir: &Path, name: &str) -> Store {
        Store::create(&dir.join(name)).unwrap()
    }

    fn line(hour: usize, minute: usize, ms: f64, sql: &str) -> String {
        format!(
            r#"{{"timestamp":"2026-10-14 {hour:02}:{minute:02}:00.000 UTC","pid":1,"session_id":"a.1","line_num":1,"error_severity":"LOG","message":"duration: {ms} ms  statement: {sql}","backend_type":"client backend"}}"#
        ) + "\n"
    }

    /// `n` statements spread over three hours.
    fn log_of(range: std::ops::Range<usize>) -> String {
        range
            .map(|i| {
                line(
                    10 + i % 3,
                    i % 60,
                    f64::from(u8::try_from(i % 200).unwrap()),
                    &format!("SELECT {}", i % 7),
                )
            })
            .collect()
    }

    fn gz(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn run(store: &mut Store, paths: &[&Path], force: bool) -> Vec<FileOutcome> {
        let paths: Vec<PathBuf> = paths.iter().map(|p| p.to_path_buf()).collect();
        ingest(store, &paths, &pipeline::Options::new(), force)
    }

    fn statuses(outcomes: &[FileOutcome]) -> Vec<&Status> {
        outcomes.iter().map(|o| &o.status).collect()
    }

    fn inputs(store: &Store) -> Vec<Input> {
        store.inputs_in(i64::MIN, i64::MAX).unwrap()
    }

    #[test]
    fn an_empty_file_is_stored_and_called_empty() {
        let d = dir("empty");
        let a = d.join("a.json");
        fs::write(&a, "").unwrap();
        let mut store = store_in(&d, "s.db");
        let out = run(&mut store, &[&a], false);
        assert_eq!(statuses(&out), [&Status::Ingested], "{out:?}");
        let warnings = out[0].diag.as_ref().unwrap().warnings.join("\n");
        assert!(warnings.contains("file is empty"), "{warnings}");
        assert_eq!(
            statuses(&run(&mut store, &[&a], false)),
            [&Status::Unchanged]
        );
    }

    #[test]
    fn second_ingest_of_the_same_file_is_unchanged_without_reading_it() {
        let d = dir("twice");
        let a = d.join("a.json");
        fs::write(&a, log_of(0..30)).unwrap();
        let mut store = store_in(&d, "s.db");
        assert_eq!(
            statuses(&run(&mut store, &[&a], false)),
            [&Status::Ingested]
        );
        let before = store.slices(i64::MIN, i64::MAX).unwrap();
        assert_eq!(
            statuses(&run(&mut store, &[&a], false)),
            [&Status::Unchanged]
        );
        assert_eq!(store.slices(i64::MIN, i64::MAX).unwrap(), before);
        assert_eq!(inputs(&store).len(), 1);
    }

    #[test]
    fn same_content_under_another_name_or_compression_is_unchanged() {
        let d = dir("rename");
        let text = log_of(0..30);
        let a = d.join("a.json");
        fs::write(&a, &text).unwrap();
        let mut store = store_in(&d, "s.db");
        run(&mut store, &[&a], false);
        let b = d.join("b.json");
        fs::copy(&a, &b).unwrap();
        let c = d.join("c.json.gz");
        fs::write(&c, gz(text.as_bytes())).unwrap();
        assert_eq!(
            statuses(&run(&mut store, &[&b, &c], false)),
            [&Status::Unchanged, &Status::Unchanged]
        );
        let all = inputs(&store);
        assert_eq!(all.len(), 1);
        assert!(all[0].path.ends_with("c.json.gz"), "{}", all[0].path);
    }

    #[test]
    fn a_touched_file_with_the_same_bytes_is_unchanged() {
        let d = dir("touched");
        let a = d.join("a.json");
        fs::write(&a, log_of(0..30)).unwrap();
        let mut store = store_in(&d, "s.db");
        run(&mut store, &[&a], false);
        fs::File::options()
            .write(true)
            .open(&a)
            .unwrap()
            .set_modified(SystemTime::now() + Duration::from_secs(120))
            .unwrap();
        assert_eq!(
            statuses(&run(&mut store, &[&a], false)),
            [&Status::Unchanged]
        );
        assert_eq!(inputs(&store).len(), 1);
    }

    #[test]
    fn a_grown_file_replaces_its_earlier_version() {
        let d = dir("grown");
        let a = d.join("a.json");
        fs::write(&a, log_of(0..30)).unwrap();
        let mut store = store_in(&d, "s.db");
        run(&mut store, &[&a], false);
        fs::write(&a, log_of(0..50)).unwrap();
        assert_eq!(
            statuses(&run(&mut store, &[&a], false)),
            [&Status::Replaced(1)]
        );
        let all = inputs(&store);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].records, 50);
    }

    #[test]
    fn growth_that_cuts_a_line_in_half_still_matches_a_fresh_ingest() {
        let d = dir("halfline");
        let full = log_of(0..50);
        let cut = full.len() - 40;
        let a = d.join("a.json");
        fs::write(&a, &full[..cut]).unwrap();
        let mut grown = store_in(&d, "grown.db");
        run(&mut grown, &[&a], false);
        fs::write(&a, &full).unwrap();
        assert_eq!(
            statuses(&run(&mut grown, &[&a], false)),
            [&Status::Replaced(1)]
        );
        let b = d.join("b.json");
        fs::write(&b, &full).unwrap();
        let mut fresh = store_in(&d, "fresh.db");
        run(&mut fresh, &[&b], false);
        assert_eq!(
            grown.slices(i64::MIN, i64::MAX).unwrap(),
            fresh.slices(i64::MIN, i64::MAX).unwrap()
        );
    }

    #[test]
    fn an_unrelated_file_with_the_same_start_is_a_new_input() {
        let d = dir("lookalike");
        let a = d.join("a.json");
        fs::write(&a, log_of(0..10)).unwrap();
        let mut store = store_in(&d, "s.db");
        run(&mut store, &[&a], false);
        let b = d.join("b.json");
        let mut text = log_of(0..3);
        text.push_str(&log_of(100..130));
        fs::write(&b, text).unwrap();
        assert_eq!(
            statuses(&run(&mut store, &[&b], false)),
            [&Status::Ingested]
        );
        assert_eq!(inputs(&store).len(), 2);
    }

    #[test]
    fn force_replaces_rows_of_a_known_file() {
        let d = dir("force");
        let a = d.join("a.json");
        fs::write(&a, log_of(0..30)).unwrap();
        let mut store = store_in(&d, "s.db");
        run(&mut store, &[&a], false);
        assert_eq!(
            statuses(&run(&mut store, &[&a], true)),
            [&Status::Replaced(1)]
        );
        assert_eq!(inputs(&store).len(), 1);
    }

    #[test]
    fn a_binary_file_fails_alone() {
        let d = dir("binary");
        let bad = d.join("bad.log");
        fs::write(&bad, [0u8; 64]).unwrap();
        let good = d.join("good.json");
        fs::write(&good, log_of(0..10)).unwrap();
        let mut store = store_in(&d, "s.db");
        let out = run(&mut store, &[&bad, &good], false);
        assert!(
            matches!(out[0].status, Status::Failed(_)),
            "{:?}",
            out[0].status
        );
        assert_eq!(out[1].status, Status::Ingested);
        assert_eq!(inputs(&store).len(), 1);
    }

    #[test]
    fn input_that_decodes_to_nothing_is_not_stored() {
        let d = dir("nothing");
        let a = d.join("a.csv");
        fs::write(&a, "this is not csv\nnor is this\n").unwrap();
        let mut store = store_in(&d, "s.db");
        let options = pipeline::Options {
            format: Some(Format::Json),
            ..pipeline::Options::new()
        };
        let out = ingest(&mut store, &[a], &options, false);
        let Status::Failed(reason) = &out[0].status else {
            panic!("{:?}", out[0].status);
        };
        assert!(reason.contains("nothing could be decoded"), "{reason}");
        assert_eq!(inputs(&store), Vec::<Input>::new());
    }

    #[test]
    fn thread_count_does_not_change_the_stored_slices() {
        let d = dir("threads");
        let a = d.join("a.json");
        fs::write(&a, log_of(0..400)).unwrap();
        let slices = |threads: usize, name: &str| {
            let mut store = store_in(&d, name);
            let options = pipeline::Options {
                threads: Some(threads),
                batch_size: 7,
                ..pipeline::Options::new()
            };
            ingest(&mut store, std::slice::from_ref(&a), &options, false);
            store.slices(i64::MIN, i64::MAX).unwrap()
        };
        assert_eq!(slices(1, "one.db"), slices(4, "four.db"));
    }

    #[test]
    fn an_older_copy_of_a_stored_file_warns_about_double_counting() {
        let d = dir("older-copy");
        let current = d.join("current.json");
        fs::write(&current, log_of(0..50)).unwrap();
        let mut store = store_in(&d, "s.db");
        run(&mut store, &[&current], false);
        let snapshot = d.join("snapshot.json");
        fs::write(&snapshot, log_of(0..30)).unwrap();
        let out = run(&mut store, &[&snapshot], false);
        assert_eq!(out[0].status, Status::Ingested);
        let warnings = out[0]
            .diag
            .as_ref()
            .map(|d| d.warnings.join("\n"))
            .unwrap_or_default();
        assert!(warnings.contains("overlaps"), "{warnings}");
        assert!(warnings.contains("% of the shorter"), "{warnings}");
        assert!(warnings.contains("current.json"), "{warnings}");
    }

    #[test]
    fn files_that_follow_each_other_do_not_warn() {
        let d = dir("adjacent");
        let early: String = (0..30).map(|m| line(10, m, 1.0, "SELECT 1")).collect();
        let late: String = (0..30).map(|m| line(12, m, 1.0, "SELECT 1")).collect();
        let (a, b) = (d.join("a.json"), d.join("b.json"));
        fs::write(&a, early).unwrap();
        fs::write(&b, late).unwrap();
        let mut store = store_in(&d, "s.db");
        let out = run(&mut store, &[&a, &b], false);
        for outcome in &out {
            assert_eq!(outcome.status, Status::Ingested);
            let warnings = outcome
                .diag
                .as_ref()
                .map(|d| d.warnings.join("\n"))
                .unwrap_or_default();
            assert!(!warnings.contains("overlaps"), "{warnings}");
        }
    }

    #[test]
    fn a_second_ingest_waits_then_reports_a_lock() {
        let d = dir("lock");
        let a = d.join("a.json");
        fs::write(&a, log_of(0..30)).unwrap();
        let mut first = store_in(&d, "s.db");
        let mut second = Store::open_existing(&d.join("s.db")).unwrap();
        second.conn.busy_timeout(Duration::from_millis(50)).unwrap();
        first.begin_ingest().unwrap();
        let out = run(&mut second, &[&a], false);
        let Status::Failed(reason) = &out[0].status else {
            panic!("{:?}", out[0].status);
        };
        assert!(reason.contains("locked"), "{reason}");
        first.end_ingest().unwrap();
        assert_eq!(run(&mut second, &[&a], false)[0].status, Status::Ingested);
    }

    #[test]
    fn ingest_reports_how_many_lines_were_skipped() {
        let d = dir("skipped");
        let log = d.join("a.log");
        fs::copy(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/logs/sample.log"),
            &log,
        )
        .unwrap();
        let mut store = store_in(&d, "s.db");
        let options = pipeline::Options {
            prefix: Some("%m [%p] ".to_owned()),
            ..pipeline::Options::new()
        };
        let out = ingest(&mut store, std::slice::from_ref(&log), &options, false);
        assert_eq!(out[0].status, Status::Ingested, "{:?}", out[0].status);
        assert!(out[0].records > 0);
        assert!(out[0].skipped > 0, "{:?}", out[0]);
    }

    #[test]
    fn a_different_prefix_on_a_known_file_points_at_force() {
        let d = dir("prefix-hint");
        let log = d.join("a.log");
        fs::copy(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/logs/sample.log"),
            &log,
        )
        .unwrap();
        let mut store = store_in(&d, "s.db");
        let first = pipeline::Options {
            prefix: Some("%m [%p] ".to_owned()),
            ..pipeline::Options::new()
        };
        ingest(&mut store, std::slice::from_ref(&log), &first, false);
        let right = pipeline::Options {
            prefix: Some("%m [%p] %q%u@%d/%a %Q ".to_owned()),
            ..pipeline::Options::new()
        };
        let out = ingest(&mut store, std::slice::from_ref(&log), &right, false);
        assert_eq!(out[0].status, Status::Unchanged);
        let hint = out[0]
            .diag
            .as_ref()
            .map(|d| d.warnings.join("\n"))
            .unwrap_or_default();
        assert!(hint.contains("--force"), "{hint}");
        assert!(hint.contains("%m [%p] "), "{hint}");
        // The same settings again say nothing.
        let again = ingest(&mut store, std::slice::from_ref(&log), &first, false);
        assert!(again[0].diag.is_none(), "{:?}", again[0].diag);
    }
}
