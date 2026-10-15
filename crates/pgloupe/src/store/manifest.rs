//! The manifest of ingested inputs and the hourly slices stored with them.

use std::collections::BTreeMap;

use std::io::{BufWriter, Read as _};

use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use rayon::prelude::*;
use rusqlite::{OptionalExtension as _, Row, params};
use serde::{Deserialize, Serialize};

use super::{Store, StoreError, sql_error};
use crate::analyze::{acc::Accumulators, diagnostics::ShapeRow, report::FileReport};

/// What the store keeps of a file's diagnostics, so a report can show them without the log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredDiag {
    pub lines: u64,
    pub warnings: Vec<String>,
    pub shapes: Vec<ShapeRow>,
}

/// One row of the manifest: a log file whose statistics are in the store.
#[derive(Debug, Clone, PartialEq)]
pub struct Input {
    /// Ignored by [`Store::commit`], which assigns it.
    pub id: i64,
    /// XXH3-128 of the decompressed bytes.
    pub hash: u128,
    /// Length of the decompressed bytes.
    pub size: u64,
    /// The path the file had when last seen.
    pub path: String,
    /// Size on disk and modification time (Unix seconds) when last seen.
    pub disk_size: u64,
    pub mtime: i64,
    pub format: String,
    pub records: u64,
    pub skipped: u64,
    pub first_ts: Option<i64>,
    pub last_ts: Option<i64>,
    pub summary: FileReport,
    pub diag: StoredDiag,
    pub ingested_at: i64,
}

/// Slices decoded at a time when reading: enough to keep every core busy, few enough to bound
/// memory.
const READ_BATCH: usize = 16;

/// A slice as it is stored: JSON, deflated. Repeated field names and table names compress away.
fn encode(acc: &Accumulators) -> Result<Vec<u8>, StoreError> {
    // serde_json writes many tiny pieces; buffering them lets the compressor see big blocks.
    let mut writer =
        BufWriter::with_capacity(64 * 1024, GzEncoder::new(Vec::new(), Compression::fast()));
    serde_json::to_writer(&mut writer, acc)?;
    let encoder = writer
        .into_inner()
        .map_err(std::io::IntoInnerError::into_error)?;
    Ok(encoder.finish()?)
}

fn decode(blob: &[u8]) -> Result<Accumulators, StoreError> {
    let mut json = Vec::with_capacity(blob.len().saturating_mul(8));
    GzDecoder::new(blob).read_to_end(&mut json)?;
    Ok(serde_json::from_slice(&json)?)
}

const COLUMNS: &str = "id, hash, size, path, disk_size, mtime, format, records, skipped, \
                       first_ts, last_ts, summary, diag, ingested_at";

fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn to_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

fn unblob(bytes: &[u8]) -> Result<u128, StoreError> {
    <[u8; 16]>::try_from(bytes)
        .map(u128::from_be_bytes)
        .map_err(|_err| StoreError::BadHash)
}

/// A manifest row as SQLite hands it over, before the JSON columns are parsed.
struct Raw {
    id: i64,
    hash: Vec<u8>,
    size: i64,
    path: String,
    disk_size: i64,
    mtime: i64,
    format: String,
    records: i64,
    skipped: i64,
    first_ts: Option<i64>,
    last_ts: Option<i64>,
    summary: String,
    diag: String,
    ingested_at: i64,
}

impl Raw {
    fn read(row: &Row<'_>) -> rusqlite::Result<Raw> {
        Ok(Raw {
            id: row.get(0)?,
            hash: row.get(1)?,
            size: row.get(2)?,
            path: row.get(3)?,
            disk_size: row.get(4)?,
            mtime: row.get(5)?,
            format: row.get(6)?,
            records: row.get(7)?,
            skipped: row.get(8)?,
            first_ts: row.get(9)?,
            last_ts: row.get(10)?,
            summary: row.get(11)?,
            diag: row.get(12)?,
            ingested_at: row.get(13)?,
        })
    }

    fn into_input(self) -> Result<Input, StoreError> {
        Ok(Input {
            id: self.id,
            hash: unblob(&self.hash)?,
            size: to_u64(self.size),
            path: self.path,
            disk_size: to_u64(self.disk_size),
            mtime: self.mtime,
            format: self.format,
            records: to_u64(self.records),
            skipped: to_u64(self.skipped),
            first_ts: self.first_ts,
            last_ts: self.last_ts,
            summary: serde_json::from_str(&self.summary)?,
            diag: serde_json::from_str(&self.diag)?,
            ingested_at: self.ingested_at,
        })
    }
}

/// Decodes `batch` in parallel and hands the slices to `visit` in order, emptying `batch`.
fn visit_decoded(
    batch: &mut Vec<Vec<u8>>,
    visit: &mut impl FnMut(Accumulators),
) -> Result<(), StoreError> {
    let decoded: Vec<Result<Accumulators, StoreError>> =
        batch.par_iter().map(|blob| decode(blob)).collect();
    batch.clear();
    for slice in decoded {
        visit(slice?);
    }
    Ok(())
}

impl Store {
    fn inputs_where(
        &self,
        condition: &str,
        params: impl rusqlite::Params,
    ) -> Result<Vec<Input>, StoreError> {
        let sql = format!("SELECT {COLUMNS} FROM inputs WHERE {condition} ORDER BY id");
        let rows = (|| {
            let mut statement = self.conn.prepare(&sql)?;
            statement
                .query_map(params, Raw::read)?
                .collect::<rusqlite::Result<Vec<Raw>>>()
        })()
        .map_err(|e| sql_error(e, &self.path))?;
        rows.into_iter().map(Raw::into_input).collect()
    }

    /// The input whose decompressed bytes hash to `hash`.
    ///
    /// # Errors
    ///
    /// Fails on database errors or unreadable stored data.
    pub fn input_by_hash(&self, hash: u128) -> Result<Option<Input>, StoreError> {
        let blob = hash.to_be_bytes().to_vec();
        Ok(self.inputs_where("hash = ?1", [blob])?.into_iter().next())
    }

    /// An input last seen at `path` with this on-disk size and modification time.
    ///
    /// # Errors
    ///
    /// Fails on database errors or unreadable stored data.
    pub fn input_by_stat(
        &self,
        path: &str,
        disk_size: u64,
        mtime: i64,
    ) -> Result<Option<Input>, StoreError> {
        Ok(self
            .inputs_where(
                "path = ?1 AND disk_size = ?2 AND mtime = ?3",
                params![path, to_i64(disk_size), mtime],
            )?
            .into_iter()
            .next())
    }

    /// The distinct decompressed sizes of all inputs, ascending.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub fn input_sizes(&self) -> Result<Vec<u64>, StoreError> {
        (|| {
            let mut statement = self
                .conn
                .prepare("SELECT DISTINCT size FROM inputs ORDER BY size")?;
            statement
                .query_map([], |row| row.get::<_, i64>(0))?
                .map(|size| size.map(to_u64))
                .collect::<rusqlite::Result<Vec<u64>>>()
        })()
        .map_err(|e| sql_error(e, &self.path))
    }

    /// The id of the input that is exactly `size` bytes with hash `hash`.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub fn input_with_prefix(&self, size: u64, hash: u128) -> Result<Option<i64>, StoreError> {
        self.conn
            .query_row(
                "SELECT id FROM inputs WHERE size = ?1 AND hash = ?2",
                params![to_i64(size), hash.to_be_bytes().to_vec()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| sql_error(e, &self.path))
    }

    /// Records a new path and stat for an input seen again.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub fn touch(
        &mut self,
        id: i64,
        path: &str,
        disk_size: u64,
        mtime: i64,
    ) -> Result<(), StoreError> {
        self.conn
            .execute(
                "UPDATE inputs SET path = ?2, disk_size = ?3, mtime = ?4, \
                 summary = json_set(summary, '$.path', ?2) WHERE id = ?1",
                params![id, path, to_i64(disk_size), mtime],
            )
            .map(|_rows| ())
            .map_err(|e| sql_error(e, &self.path))
    }

    /// Adds `input` with its hourly `slices` and deletes the inputs in `replace` (and their
    /// slices), all in one transaction. Returns the new input's id.
    ///
    /// # Errors
    ///
    /// Fails, changing nothing, on database errors (including a hash that is already stored and
    /// not being replaced).
    pub fn commit(
        &mut self,
        input: &Input,
        slices: &BTreeMap<i64, Accumulators>,
        replace: &[i64],
    ) -> Result<i64, StoreError> {
        let summary = serde_json::to_string(&input.summary)?;
        let diag = serde_json::to_string(&input.diag)?;
        let slices: Vec<(&i64, &Accumulators)> = slices.iter().collect();
        let data = slices
            .par_iter()
            .map(|(hour, acc)| Ok((**hour, encode(acc)?)))
            .collect::<Result<Vec<_>, StoreError>>()?;
        let path = self.path.clone();
        let tx = self.conn.savepoint().map_err(|e| sql_error(e, &path))?;
        (|| {
            for id in replace {
                tx.execute("DELETE FROM inputs WHERE id = ?1", [id])?;
            }
            tx.execute(
                "INSERT INTO inputs (hash, size, path, disk_size, mtime, format, records, skipped, \
                 first_ts, last_ts, summary, diag, ingested_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    input.hash.to_be_bytes().to_vec(),
                    to_i64(input.size),
                    input.path,
                    to_i64(input.disk_size),
                    input.mtime,
                    input.format,
                    to_i64(input.records),
                    to_i64(input.skipped),
                    input.first_ts,
                    input.last_ts,
                    summary,
                    diag,
                    input.ingested_at,
                ],
            )?;
            let id = tx.last_insert_rowid();
            {
                let mut insert =
                    tx.prepare("INSERT INTO slices (input_id, hour, data) VALUES (?1, ?2, ?3)")?;
                for (hour, blob) in &data {
                    insert.execute(params![id, hour, blob])?;
                }
            }
            tx.commit()?;
            Ok(id)
        })()
        .map_err(|e| sql_error(e, &path))
    }

    /// Inputs that have at least one slice in `[since, until)` (hour starts, Unix seconds).
    ///
    /// # Errors
    ///
    /// Fails on database errors or unreadable stored data.
    pub fn inputs_in(&self, since: i64, until: i64) -> Result<Vec<Input>, StoreError> {
        self.inputs_where(
            "id IN (SELECT DISTINCT input_id FROM slices WHERE hour >= ?1 AND hour < ?2)",
            [since, until],
        )
    }

    /// Calls `visit` with each slice whose hour is in `[since, until)`, ordered by hour, then
    /// input. Slices are read one at a time, so memory stays at one slice plus what `visit` keeps.
    ///
    /// # Errors
    ///
    /// Fails on database errors or unreadable stored data.
    pub fn for_each_slice(
        &self,
        since: i64,
        until: i64,
        mut visit: impl FnMut(Accumulators),
    ) -> Result<(), StoreError> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT data FROM slices WHERE hour >= ?1 AND hour < ?2 ORDER BY hour, input_id",
            )
            .map_err(|e| sql_error(e, &self.path))?;
        let mut rows = statement
            .query([since, until])
            .map_err(|e| sql_error(e, &self.path))?;
        let mut batch: Vec<Vec<u8>> = Vec::with_capacity(READ_BATCH);
        while let Some(row) = rows.next().map_err(|e| sql_error(e, &self.path))? {
            batch.push(row.get(0).map_err(|e| sql_error(e, &self.path))?);
            if batch.len() == READ_BATCH {
                visit_decoded(&mut batch, &mut visit)?;
            }
        }
        visit_decoded(&mut batch, &mut visit)
    }

    /// The slices whose hour is in `[since, until)`, ordered by hour, then input.
    ///
    /// # Errors
    ///
    /// Fails on database errors or unreadable stored data.
    pub fn slices(&self, since: i64, until: i64) -> Result<Vec<Accumulators>, StoreError> {
        let mut all = Vec::new();
        self.for_each_slice(since, until, |acc| all.push(acc))?;
        Ok(all)
    }

    /// Takes the store's write lock until [`Store::end_ingest`], so that what an ingest looked up
    /// (known hashes, earlier versions) cannot change before it writes. Another writer waits for
    /// the busy timeout and then gets [`StoreError::Locked`].
    ///
    /// # Errors
    ///
    /// Fails with `Locked` when another process holds the lock.
    pub fn begin_ingest(&mut self) -> Result<(), StoreError> {
        self.conn
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| sql_error(e, &self.path))
    }

    /// Releases the lock taken by [`Store::begin_ingest`], keeping what was committed meanwhile.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub fn end_ingest(&mut self) -> Result<(), StoreError> {
        let committed = self.conn.execute_batch("COMMIT");
        if committed.is_err() {
            // A failed COMMIT leaves the transaction open; the next file needs a fresh one.
            drop(self.conn.execute_batch("ROLLBACK"));
        }
        committed.map_err(|e| sql_error(e, &self.path))
    }
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;

    use super::*;
    use crate::log::{LogRecord, Severity};

    fn temp_store(name: &str) -> (Store, std::path::PathBuf) {
        let path =
            std::env::temp_dir().join(format!("pgloupe-manifest-{}-{name}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        (Store::create(&path).unwrap(), path)
    }

    fn input(hash: u128, size: u64, path: &str) -> Input {
        Input {
            id: 0,
            hash,
            size,
            path: path.to_owned(),
            disk_size: size,
            mtime: 1_000,
            format: "json".to_owned(),
            records: 1,
            skipped: 0,
            first_ts: Some(0),
            last_ts: Some(10),
            summary: FileReport {
                path: path.to_owned(),
                bytes: size,
                format: "json".to_owned(),
                prefix: None,
                prefix_source: "n/a".to_owned(),
                prefix_match: None,
                lines: 1,
                records: 1,
                locale: None,
                skipped: 0,
            },
            diag: StoredDiag {
                lines: 1,
                warnings: vec!["w".to_owned()],
                shapes: Vec::new(),
            },
            ingested_at: 5,
        }
    }

    fn slices(hours: &[i64]) -> BTreeMap<i64, Accumulators> {
        hours
            .iter()
            .map(|hour| {
                let mut acc = Accumulators::new(100, 60);
                acc.add_record(&LogRecord::new(
                    Timestamp::from_second(*hour + 5).unwrap(),
                    Severity::Log,
                    "duration: 5.0 ms  statement: SELECT 1",
                ));
                (*hour, acc)
            })
            .collect()
    }

    #[test]
    fn commit_then_lookup_by_hash_stat_and_prefix() {
        let (mut store, path) = temp_store("lookup");
        let id = store
            .commit(&input(7, 100, "a.log"), &slices(&[0]), &[])
            .unwrap();
        let found = store.input_by_hash(7).unwrap().unwrap();
        assert_eq!(found.id, id);
        assert_eq!(
            found,
            Input {
                id,
                ..input(7, 100, "a.log")
            }
        );
        assert!(store.input_by_hash(8).unwrap().is_none());
        assert!(store.input_by_stat("a.log", 100, 1_000).unwrap().is_some());
        assert!(store.input_by_stat("a.log", 100, 1_001).unwrap().is_none());
        assert!(store.input_by_stat("b.log", 100, 1_000).unwrap().is_none());
        assert_eq!(store.input_sizes().unwrap(), vec![100]);
        assert_eq!(store.input_with_prefix(100, 7).unwrap(), Some(id));
        assert_eq!(store.input_with_prefix(100, 8).unwrap(), None);
        assert_eq!(store.input_with_prefix(99, 7).unwrap(), None);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn hashes_use_the_whole_128_bits() {
        let (mut store, path) = temp_store("wide");
        let hash = u128::MAX - 3;
        store
            .commit(&input(hash, 10, "a.log"), &slices(&[0]), &[])
            .unwrap();
        assert!(store.input_by_hash(hash).unwrap().is_some());
        assert!(store.input_by_hash(3).unwrap().is_none());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn replace_deletes_the_old_input_and_its_slices_atomically() {
        let (mut store, path) = temp_store("replace");
        let old = store
            .commit(&input(1, 100, "a.log"), &slices(&[0]), &[])
            .unwrap();
        store
            .commit(&input(2, 200, "a.log"), &slices(&[3_600]), &[old])
            .unwrap();
        assert!(store.input_by_hash(1).unwrap().is_none());
        assert!(store.input_by_hash(2).unwrap().is_some());
        let rows = store.slices(i64::MIN, i64::MAX).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].totals.first,
            Some(Timestamp::from_second(3_605).unwrap())
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_failed_commit_leaves_the_old_state() {
        let (mut store, path) = temp_store("atomic");
        store
            .commit(&input(1, 100, "a.log"), &slices(&[0]), &[])
            .unwrap();
        // Same hash again, without asking to replace it: the UNIQUE constraint refuses.
        store
            .commit(&input(1, 100, "b.log"), &slices(&[3_600]), &[])
            .unwrap_err();
        assert_eq!(store.slices(i64::MIN, i64::MAX).unwrap().len(), 1);
        assert_eq!(store.inputs_in(i64::MIN, i64::MAX).unwrap().len(), 1);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn slices_are_selected_by_hour_and_ordered() {
        let (mut store, path) = temp_store("window");
        store
            .commit(&input(1, 10, "a.log"), &slices(&[0, 3_600, 7_200]), &[])
            .unwrap();
        store
            .commit(&input(2, 20, "b.log"), &slices(&[3_600]), &[])
            .unwrap();
        assert_eq!(store.slices(3_600, 7_200).unwrap().len(), 2);
        assert_eq!(store.slices(0, 3_600).unwrap().len(), 1);
        assert_eq!(store.slices(7_201, i64::MAX).unwrap().len(), 0);
        assert_eq!(store.slices(i64::MIN, i64::MAX).unwrap().len(), 4);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn inputs_in_lists_only_inputs_with_slices_in_the_window() {
        let (mut store, path) = temp_store("inputs-in");
        store
            .commit(&input(1, 10, "a.log"), &slices(&[0]), &[])
            .unwrap();
        store
            .commit(&input(2, 20, "b.log"), &slices(&[7_200]), &[])
            .unwrap();
        let hit = store.inputs_in(7_200, 10_800).unwrap();
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].path, "b.log");
        assert_eq!(store.inputs_in(i64::MIN, i64::MAX).unwrap().len(), 2);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn touch_updates_path_and_stat() {
        let (mut store, path) = temp_store("touch");
        let id = store
            .commit(&input(1, 10, "a.log"), &slices(&[0]), &[])
            .unwrap();
        store.touch(id, "moved.log", 11, 2_000).unwrap();
        assert!(
            store
                .input_by_stat("moved.log", 11, 2_000)
                .unwrap()
                .is_some()
        );
        assert!(store.input_by_stat("a.log", 10, 1_000).unwrap().is_none());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn touch_also_moves_the_path_shown_in_reports() {
        let (mut store, path) = temp_store("touch-summary");
        let mut stored = input(1, 10, "a.log");
        stored.summary.path = "a.log".to_owned();
        let id = store.commit(&stored, &slices(&[0]), &[]).unwrap();
        store.touch(id, "moved.log", 10, 1_000).unwrap();
        let found = store.input_by_hash(1).unwrap().unwrap();
        assert_eq!(found.summary.path, "moved.log");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_failed_commit_ends_the_ingest_transaction() {
        let (mut store, path) = temp_store("failed-end");
        store.begin_ingest().unwrap();
        store
            .conn
            .execute_batch(
                "PRAGMA defer_foreign_keys = ON; \
                 INSERT INTO slices (input_id, hour, data) VALUES (999, 0, x'00');",
            )
            .unwrap();
        store.end_ingest().unwrap_err();
        store.begin_ingest().unwrap();
        store.end_ingest().unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn for_each_slice_visits_what_slices_returns() {
        let (mut store, path) = temp_store("visit");
        store
            .commit(&input(1, 10, "a.log"), &slices(&[0, 3_600]), &[])
            .unwrap();
        store
            .commit(&input(2, 20, "b.log"), &slices(&[3_600]), &[])
            .unwrap();
        let mut seen = Vec::new();
        store
            .for_each_slice(i64::MIN, i64::MAX, |acc| seen.push(acc))
            .unwrap();
        assert_eq!(seen, store.slices(i64::MIN, i64::MAX).unwrap());
        assert_eq!(seen.len(), 3);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn slices_are_stored_compressed() {
        let (mut store, path) = temp_store("compressed");
        let mut acc = Accumulators::new(1_000, 60);
        for i in 0..200 {
            acc.add_record(&LogRecord::new(
                Timestamp::from_second(5 + i).unwrap(),
                Severity::Log,
                format!(
                    "duration: 5.0 ms  statement: SELECT * FROM table_{}",
                    "x".repeat(usize::try_from(i % 7).unwrap() + 1)
                ),
            ));
        }
        let json_len = serde_json::to_string(&acc).unwrap().len();
        store
            .commit(
                &input(1, 10, "a.log"),
                &BTreeMap::from([(0, acc.clone())]),
                &[],
            )
            .unwrap();
        let (kind, stored): (String, i64) = store
            .conn
            .query_row("SELECT typeof(data), length(data) FROM slices", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(kind, "blob");
        assert!(
            usize::try_from(stored).unwrap() * 3 < json_len,
            "{stored} vs {json_len}"
        );
        assert_eq!(store.slices(i64::MIN, i64::MAX).unwrap(), vec![acc]);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn many_slices_come_back_in_hour_order() {
        let (mut store, path) = temp_store("many");
        let hours: Vec<i64> = (0..70).map(|h| h * 3_600).collect();
        store
            .commit(&input(1, 10, "a.log"), &slices(&hours), &[])
            .unwrap();
        let firsts: Vec<i64> = store
            .slices(i64::MIN, i64::MAX)
            .unwrap()
            .iter()
            .map(|acc| acc.totals.first.unwrap().as_second())
            .collect();
        let expected: Vec<i64> = hours.iter().map(|h| h + 5).collect();
        assert_eq!(firsts, expected);
        std::fs::remove_file(path).unwrap();
    }
}
