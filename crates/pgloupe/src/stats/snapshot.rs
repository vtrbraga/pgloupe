//! `pg_stat_statements` snapshots read from CSV exports.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    path::Path,
};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::error::SnapshotError;

/// Columns every export must have. `queryid` may be empty per row, but the column must exist.
const REQUIRED: [&str; 8] = [
    "snapshot_at",
    "server_version_num",
    "dbid",
    "userid",
    "queryid",
    "calls",
    "rows",
    "total_exec_time",
];

/// Columns that may be absent from an export. Two snapshots must have the same ones to be compared.
const OPTIONAL: [&str; 9] = [
    "toplevel",
    "stats_since",
    "plans",
    "total_plan_time",
    "shared_blks_hit",
    "shared_blks_read",
    "temp_blks_read",
    "temp_blks_written",
    "wal_bytes",
];

/// Identity of one `pg_stat_statements` entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key {
    pub dbid: u32,
    pub userid: u32,
    pub queryid: i64,
    pub toplevel: bool,
}

/// Cumulative counters of one entry. Times are in milliseconds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Counters {
    pub calls: u64,
    pub rows: u64,
    pub plans: u64,
    pub total_plan_time: f64,
    pub total_exec_time: f64,
    pub shared_blks_hit: u64,
    pub shared_blks_read: u64,
    pub temp_blks_read: u64,
    pub temp_blks_written: u64,
    pub wal_bytes: u64,
}

impl Counters {
    /// True if any counter is smaller than in `before`, which means the entry was reset or evicted.
    #[must_use]
    pub fn went_backwards(&self, before: &Counters) -> bool {
        self.calls < before.calls
            || self.rows < before.rows
            || self.plans < before.plans
            || self.total_plan_time < before.total_plan_time
            || self.total_exec_time < before.total_exec_time
            || self.shared_blks_hit < before.shared_blks_hit
            || self.shared_blks_read < before.shared_blks_read
            || self.temp_blks_read < before.temp_blks_read
            || self.temp_blks_written < before.temp_blks_written
            || self.wal_bytes < before.wal_bytes
    }

    /// Field-wise `self - before`. Only meaningful when `!self.went_backwards(before)`.
    #[must_use]
    pub fn minus(&self, before: &Counters) -> Counters {
        Counters {
            calls: self.calls.saturating_sub(before.calls),
            rows: self.rows.saturating_sub(before.rows),
            plans: self.plans.saturating_sub(before.plans),
            total_plan_time: self.total_plan_time - before.total_plan_time,
            total_exec_time: self.total_exec_time - before.total_exec_time,
            shared_blks_hit: self.shared_blks_hit.saturating_sub(before.shared_blks_hit),
            shared_blks_read: self
                .shared_blks_read
                .saturating_sub(before.shared_blks_read),
            temp_blks_read: self.temp_blks_read.saturating_sub(before.temp_blks_read),
            temp_blks_written: self
                .temp_blks_written
                .saturating_sub(before.temp_blks_written),
            wal_bytes: self.wal_bytes.saturating_sub(before.wal_bytes),
        }
    }
}

/// One row of the export.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Entry {
    pub datname: Option<String>,
    pub rolname: Option<String>,
    pub query: Option<String>,
    /// PG17+: when this entry's counters started.
    pub stats_since: Option<Timestamp>,
    pub counters: Counters,
}

/// One row of `pg_stat_statements_info`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Info {
    /// Entries evicted since `stats_reset`.
    pub dealloc: u64,
    pub stats_reset: Option<Timestamp>,
}

/// A point-in-time copy of `pg_stat_statements`.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub taken_at: Timestamp,
    pub server_version_num: u32,
    /// Ordered by [`Key`], so iteration is deterministic.
    pub entries: BTreeMap<Key, Entry>,
    /// Rows with an empty or zero `queryid`.
    pub skipped_no_queryid: usize,
    /// Which of the optional columns the export had; absent ones read as zero.
    pub optional_columns: BTreeSet<&'static str>,
    pub info: Option<Info>,
}

/// A CSV row as it comes off the wire; optional columns may be absent or empty.
#[derive(Debug, Deserialize)]
struct RawRow {
    snapshot_at: String,
    server_version_num: u32,
    dbid: u32,
    userid: u32,
    queryid: Option<i64>,
    calls: u64,
    rows: u64,
    total_exec_time: f64,
    toplevel: Option<String>,
    datname: Option<String>,
    rolname: Option<String>,
    query: Option<String>,
    stats_since: Option<String>,
    plans: Option<u64>,
    total_plan_time: Option<f64>,
    shared_blks_hit: Option<u64>,
    shared_blks_read: Option<u64>,
    temp_blks_read: Option<u64>,
    temp_blks_written: Option<u64>,
    wal_bytes: Option<u64>,
}

/// A `pg_stat_statements_info` CSV row; `snapshot_at` is ignored.
#[derive(Debug, Deserialize)]
struct RawInfo {
    dealloc: u64,
    stats_reset: Option<String>,
}

impl Snapshot {
    /// Reads a snapshot from a `COPY ... WITH (FORMAT csv, HEADER)` export.
    ///
    /// `file` only labels error messages.
    ///
    /// # Errors
    ///
    /// Fails if a required column is missing, a value does not parse, rows disagree on the
    /// snapshot time or server version, an entry appears twice, or there are no rows.
    pub fn from_reader(reader: impl Read, file: &str) -> Result<Self, SnapshotError> {
        let mut rdr = csv::Reader::from_reader(reader);
        let headers = rdr
            .headers()
            .map_err(|source| csv_error(file, source))?
            .clone();
        if has_column(&headers, "total_time") && !has_column(&headers, "total_exec_time") {
            return Err(SnapshotError::LegacyColumns {
                file: file.to_owned(),
            });
        }
        check_columns(&headers, file, &REQUIRED)?;

        let optional_columns: BTreeSet<&'static str> = OPTIONAL
            .iter()
            .copied()
            .filter(|column| has_column(&headers, column))
            .collect();
        let mut seen: Option<(String, u32)> = None;
        let mut entries = BTreeMap::new();
        let mut skipped_no_queryid = 0;
        let mut record = csv::StringRecord::new();
        while rdr
            .read_record(&mut record)
            .map_err(|source| csv_error(file, source))?
        {
            let line = record.position().map_or(0, csv::Position::line);
            let raw: RawRow = record
                .deserialize(Some(&headers))
                .map_err(|source| csv_error(file, source))?;
            let (at, version) =
                seen.get_or_insert_with(|| (raw.snapshot_at.clone(), raw.server_version_num));
            if *at != raw.snapshot_at {
                return Err(inconsistent(file, "snapshot_at", at, &raw.snapshot_at));
            }
            if *version != raw.server_version_num {
                return Err(inconsistent(
                    file,
                    "server_version_num",
                    &version.to_string(),
                    &raw.server_version_num.to_string(),
                ));
            }
            let Some(queryid) = raw.queryid.filter(|id| *id != 0) else {
                skipped_no_queryid += 1;
                continue;
            };
            let (key, entry) = entry_from(raw, queryid, file, line)?;
            if entries.insert(key, entry).is_some() {
                return Err(SnapshotError::DuplicateKey {
                    file: file.to_owned(),
                    dbid: key.dbid,
                    userid: key.userid,
                    queryid: key.queryid,
                    toplevel: key.toplevel,
                });
            }
        }
        let Some((at, server_version_num)) = seen else {
            return Err(SnapshotError::NoRows {
                file: file.to_owned(),
            });
        };
        // Every row has the same `snapshot_at`; the first data row is line 2.
        let taken_at =
            parse_timestamp(&at).map_err(|message| bad_value(file, 2, "snapshot_at", &message))?;
        Ok(Snapshot {
            taken_at,
            server_version_num,
            entries,
            skipped_no_queryid,
            optional_columns,
            info: None,
        })
    }

    /// Reads a snapshot from a file.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be opened or parsed; see [`Snapshot::from_reader`].
    pub fn from_path(path: &Path) -> Result<Self, SnapshotError> {
        let (file, label) = open(path)?;
        Self::from_reader(file, &label)
    }

    /// Attaches the matching `pg_stat_statements_info` row.
    #[must_use]
    pub fn with_info(mut self, info: Info) -> Self {
        self.info = Some(info);
        self
    }
}

impl Info {
    /// Reads the single data row of a `pg_stat_statements_info` export.
    ///
    /// # Errors
    ///
    /// Fails if the `dealloc` column is missing, the file has no rows, or a value does not parse.
    pub fn from_reader(reader: impl Read, file: &str) -> Result<Self, SnapshotError> {
        let mut rdr = csv::Reader::from_reader(reader);
        let headers = rdr
            .headers()
            .map_err(|source| csv_error(file, source))?
            .clone();
        check_columns(&headers, file, &["dealloc"])?;
        let mut record = csv::StringRecord::new();
        if !rdr
            .read_record(&mut record)
            .map_err(|source| csv_error(file, source))?
        {
            return Err(SnapshotError::NoRows {
                file: file.to_owned(),
            });
        }
        let raw: RawInfo = record
            .deserialize(Some(&headers))
            .map_err(|source| csv_error(file, source))?;
        let stats_reset = raw
            .stats_reset
            .as_deref()
            .map(parse_timestamp)
            .transpose()
            .map_err(|message| bad_value(file, 2, "stats_reset", &message))?;
        Ok(Info {
            dealloc: raw.dealloc,
            stats_reset,
        })
    }

    /// Reads an info export from a file.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be opened or parsed; see [`Info::from_reader`].
    pub fn from_path(path: &Path) -> Result<Self, SnapshotError> {
        let (file, label) = open(path)?;
        Self::from_reader(file, &label)
    }
}

fn open(path: &Path) -> Result<(File, String), SnapshotError> {
    let label = path.display().to_string();
    match File::open(path) {
        Ok(file) => Ok((file, label)),
        Err(source) => Err(SnapshotError::Io {
            file: label,
            source,
        }),
    }
}

fn entry_from(
    raw: RawRow,
    queryid: i64,
    file: &str,
    line: u64,
) -> Result<(Key, Entry), SnapshotError> {
    let toplevel = parse_toplevel(raw.toplevel.as_deref())
        .map_err(|message| bad_value(file, line, "toplevel", &message))?;
    let stats_since = raw
        .stats_since
        .as_deref()
        .map(parse_timestamp)
        .transpose()
        .map_err(|message| bad_value(file, line, "stats_since", &message))?;
    let key = Key {
        dbid: raw.dbid,
        userid: raw.userid,
        queryid,
        toplevel,
    };
    let entry = Entry {
        datname: raw.datname,
        rolname: raw.rolname,
        query: raw.query,
        stats_since,
        counters: Counters {
            calls: raw.calls,
            rows: raw.rows,
            plans: raw.plans.unwrap_or(0),
            total_plan_time: raw.total_plan_time.unwrap_or(0.0),
            total_exec_time: raw.total_exec_time,
            shared_blks_hit: raw.shared_blks_hit.unwrap_or(0),
            shared_blks_read: raw.shared_blks_read.unwrap_or(0),
            temp_blks_read: raw.temp_blks_read.unwrap_or(0),
            temp_blks_written: raw.temp_blks_written.unwrap_or(0),
            wal_bytes: raw.wal_bytes.unwrap_or(0),
        },
    };
    Ok((key, entry))
}

fn has_column(headers: &csv::StringRecord, name: &str) -> bool {
    headers.iter().any(|header| header == name)
}

fn check_columns(
    headers: &csv::StringRecord,
    file: &str,
    required: &[&'static str],
) -> Result<(), SnapshotError> {
    for &column in required {
        if !has_column(headers, column) {
            return Err(SnapshotError::MissingColumn {
                file: file.to_owned(),
                column,
            });
        }
    }
    Ok(())
}

/// Postgres prints booleans as `t`/`f`; the column is absent before PG14, which means top level.
fn parse_toplevel(value: Option<&str>) -> Result<bool, String> {
    match value {
        None | Some("t" | "true") => Ok(true),
        Some("f" | "false") => Ok(false),
        Some(other) => Err(format!("toplevel {other:?} is not t or f")),
    }
}

fn parse_timestamp(text: &str) -> Result<Timestamp, String> {
    text.parse::<Timestamp>()
        .map_err(|err| format!("{text:?} is not a timestamp: {err}"))
}

fn csv_error(file: &str, source: csv::Error) -> SnapshotError {
    SnapshotError::Csv {
        file: file.to_owned(),
        source,
    }
}

fn bad_value(file: &str, line: u64, column: &str, message: &str) -> SnapshotError {
    SnapshotError::BadValue {
        file: file.to_owned(),
        line,
        message: format!("{column}: {message}"),
    }
}

fn inconsistent(file: &str, column: &'static str, first: &str, other: &str) -> SnapshotError {
    SnapshotError::Inconsistent {
        file: file.to_owned(),
        column,
        first: first.to_owned(),
        other: other.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::approx_eq;

    const HEADER: &str = "snapshot_at,server_version_num,datname,rolname,userid,dbid,toplevel,queryid,query,calls,total_exec_time,rows";
    const ROW: &str =
        "2026-10-14 10:00:00+00,170011,pagila,postgres,10,16387,t,1,SELECT 1,100,50.5,100";

    fn read_with(header: &str, rows: &[&str]) -> Result<Snapshot, SnapshotError> {
        let mut text = header.to_owned();
        for row in rows {
            text.push('\n');
            text.push_str(row);
        }
        Snapshot::from_reader(text.as_bytes(), "test.csv")
    }

    fn read(rows: &[&str]) -> Result<Snapshot, SnapshotError> {
        read_with(HEADER, rows)
    }

    fn key(queryid: i64) -> Key {
        Key {
            dbid: 16387,
            userid: 10,
            queryid,
            toplevel: true,
        }
    }

    fn row_with_queryid(queryid: i64) -> String {
        format!(
            "2026-10-14 10:00:00+00,170011,pagila,postgres,10,16387,t,{queryid},SELECT 1,1,0.1,1"
        )
    }

    #[test]
    fn reads_one_entry() {
        let snap = read(&[ROW]).unwrap();
        assert_eq!(snap.server_version_num, 170_011);
        assert_eq!(
            snap.taken_at,
            "2026-10-14T10:00:00Z".parse::<Timestamp>().unwrap()
        );
        let entry = &snap.entries[&key(1)];
        assert_eq!(entry.datname.as_deref(), Some("pagila"));
        assert_eq!(entry.rolname.as_deref(), Some("postgres"));
        assert_eq!(entry.query.as_deref(), Some("SELECT 1"));
        assert_eq!(entry.counters.calls, 100);
        assert_eq!(entry.counters.rows, 100);
        assert!(approx_eq(entry.counters.total_exec_time, 50.5));
        assert_eq!(entry.counters.plans, 0);
        assert_eq!(entry.stats_since, None);
        assert!(snap.info.is_none());
    }

    #[test]
    fn matches_columns_by_name_and_ignores_unknown_ones() {
        let header =
            "extra,rows,calls,total_exec_time,queryid,dbid,userid,snapshot_at,server_version_num";
        let row = "zzz,5,7,1.5,42,1,10,2026-10-14 10:00:00+00,170011";
        let snap = read_with(header, &[row]).unwrap();
        let entry = &snap.entries[&Key {
            dbid: 1,
            userid: 10,
            queryid: 42,
            toplevel: true,
        }];
        assert_eq!((entry.counters.calls, entry.counters.rows), (7, 5));
        assert_eq!(entry.query, None);
    }

    #[test]
    fn toplevel_flag_splits_entries_with_the_same_queryid() {
        let nested = "2026-10-14 10:00:00+00,170011,pagila,postgres,10,16387,f,1,SELECT 1,3,0.5,3";
        let snap = read(&[ROW, nested]).unwrap();
        assert_eq!(snap.entries.len(), 2);
        let nested_key = Key {
            toplevel: false,
            ..key(1)
        };
        assert_eq!(snap.entries[&nested_key].counters.calls, 3);
    }

    #[test]
    fn reads_optional_counters_and_stats_since_when_present() {
        let header = format!(
            "{HEADER},plans,total_plan_time,shared_blks_hit,shared_blks_read,temp_blks_read,temp_blks_written,wal_bytes,stats_since"
        );
        let row = format!("{ROW},2,1.25,10,20,30,40,5000,2026-10-14 09:00:00+00");
        let snap = read_with(&header, &[row.as_str()]).unwrap();
        let entry = &snap.entries[&key(1)];
        let c = entry.counters;
        assert_eq!(
            (
                c.plans,
                c.shared_blks_hit,
                c.shared_blks_read,
                c.temp_blks_read,
                c.temp_blks_written,
                c.wal_bytes
            ),
            (2, 10, 20, 30, 40, 5000)
        );
        assert!(approx_eq(c.total_plan_time, 1.25));
        assert_eq!(
            entry.stats_since,
            Some("2026-10-14T09:00:00Z".parse().unwrap())
        );
    }

    #[test]
    fn empty_optional_cells_count_as_zero() {
        let header = format!("{HEADER},plans,wal_bytes,stats_since");
        let row = format!("{ROW},,,");
        let snap = read_with(&header, &[row.as_str()]).unwrap();
        let entry = &snap.entries[&key(1)];
        assert_eq!(entry.counters.plans, 0);
        assert_eq!(entry.counters.wal_bytes, 0);
        assert_eq!(entry.stats_since, None);
    }

    #[test]
    fn missing_required_column_is_named() {
        let header = "snapshot_at,server_version_num,userid,dbid,queryid,rows,total_exec_time";
        let err = read_with(header, &[]).unwrap_err();
        assert!(
            matches!(
                err,
                SnapshotError::MissingColumn {
                    column: "calls",
                    ..
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn empty_file_names_a_missing_column() {
        let err = Snapshot::from_reader("".as_bytes(), "empty.csv").unwrap_err();
        assert!(matches!(err, SnapshotError::MissingColumn { .. }), "{err}");
        assert!(err.to_string().contains("empty.csv"), "{err}");
    }

    #[test]
    fn pre_pg13_total_time_column_is_explained() {
        let header = "snapshot_at,server_version_num,userid,dbid,queryid,calls,rows,total_time";
        let err = read_with(header, &[]).unwrap_err();
        assert!(matches!(err, SnapshotError::LegacyColumns { .. }), "{err}");
        assert!(err.to_string().contains("PG13"), "{err}");
    }

    #[test]
    fn rows_without_queryid_are_counted_not_keyed() {
        let empty = "2026-10-14 10:00:00+00,170011,pagila,postgres,10,16387,t,,<insufficient privilege>,1,0.1,1";
        let zero = "2026-10-14 10:00:00+00,170011,pagila,postgres,10,16387,t,0,SELECT 2,1,0.1,1";
        let snap = read(&[ROW, empty, zero]).unwrap();
        assert_eq!(snap.entries.len(), 1);
        assert_eq!(snap.skipped_no_queryid, 2);
    }

    #[test]
    fn duplicate_key_is_an_error() {
        let err = read(&[ROW, ROW]).unwrap_err();
        assert!(
            matches!(err, SnapshotError::DuplicateKey { queryid: 1, .. }),
            "{err}"
        );
    }

    #[test]
    fn rows_must_agree_on_snapshot_time_and_version() {
        let later = "2026-10-14 10:00:01+00,170011,pagila,postgres,10,16387,t,2,SELECT 2,1,0.1,1";
        let err = read(&[ROW, later]).unwrap_err();
        assert!(
            matches!(
                err,
                SnapshotError::Inconsistent {
                    column: "snapshot_at",
                    ..
                }
            ),
            "{err}"
        );
        let other_version =
            "2026-10-14 10:00:00+00,160005,pagila,postgres,10,16387,t,2,SELECT 2,1,0.1,1";
        let err = read(&[ROW, other_version]).unwrap_err();
        assert!(
            matches!(
                err,
                SnapshotError::Inconsistent {
                    column: "server_version_num",
                    ..
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn header_only_file_has_no_rows() {
        let err = read(&[]).unwrap_err();
        assert!(matches!(err, SnapshotError::NoRows { .. }), "{err}");
    }

    #[test]
    fn bad_number_reports_file_and_line() {
        let row = "2026-10-14 10:00:00+00,170011,pagila,postgres,10,16387,t,1,SELECT 1,abc,0.1,1";
        let err = read(&[row]).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("test.csv"), "{message}");
        assert!(message.contains("line: 2"), "{message}");
    }

    #[test]
    fn bad_toplevel_value_is_reported_with_line() {
        let row = "2026-10-14 10:00:00+00,170011,pagila,postgres,10,16387,x,1,SELECT 1,1,0.1,1";
        let err = read(&[ROW, row]).unwrap_err();
        assert!(
            matches!(err, SnapshotError::BadValue { line: 3, .. }),
            "{err}"
        );
    }

    #[test]
    fn quoted_multiline_query_survives() {
        let row = "2026-10-14 10:00:00+00,170011,pagila,postgres,10,16387,t,1,\"SELECT a,\n       b\nFROM \"\"t\"\"\",1,0.1,1";
        let snap = read(&[row]).unwrap();
        assert_eq!(
            snap.entries[&key(1)].query.as_deref(),
            Some("SELECT a,\n       b\nFROM \"t\"")
        );
    }

    #[test]
    fn crlf_and_bom_are_accepted() {
        let text = format!("\u{feff}{HEADER}\r\n{ROW}\r\n");
        let snap = Snapshot::from_reader(text.as_bytes(), "t.csv").unwrap();
        assert_eq!(snap.entries.len(), 1);
    }

    #[test]
    fn timestamps_accept_other_offsets_and_missing_fractions() {
        let row = "2026-10-14 07:00:00-03,170011,pagila,postgres,10,16387,t,1,SELECT 1,1,0.1,1";
        let snap = read(&[row]).unwrap();
        assert_eq!(
            snap.taken_at,
            "2026-10-14T10:00:00Z".parse::<Timestamp>().unwrap()
        );
    }

    #[test]
    fn queryid_extremes_round_trip() {
        let low = row_with_queryid(i64::MIN);
        let high = row_with_queryid(i64::MAX);
        let snap = read(&[low.as_str(), high.as_str()]).unwrap();
        assert!(snap.entries.contains_key(&key(i64::MIN)));
        assert!(snap.entries.contains_key(&key(i64::MAX)));
    }

    #[test]
    fn counters_detect_going_backwards_and_subtract() {
        let before = Counters {
            calls: 10,
            total_exec_time: 5.0,
            ..Counters::default()
        };
        let grown = Counters {
            calls: 15,
            total_exec_time: 8.0,
            ..Counters::default()
        };
        let reset = Counters {
            calls: 2,
            total_exec_time: 8.0,
            ..Counters::default()
        };
        assert!(!grown.went_backwards(&before));
        assert!(reset.went_backwards(&before));
        let diff = grown.minus(&before);
        assert_eq!(diff.calls, 5);
        assert!(approx_eq(diff.total_exec_time, 3.0));
    }

    const INFO_HEADER: &str = "snapshot_at,dealloc,stats_reset";

    fn read_info(rows: &[&str]) -> Result<Info, SnapshotError> {
        let mut text = INFO_HEADER.to_owned();
        for row in rows {
            text.push('\n');
            text.push_str(row);
        }
        Info::from_reader(text.as_bytes(), "info.csv")
    }

    #[test]
    fn info_reads_dealloc_and_stats_reset() {
        let info =
            read_info(&["2026-10-14 10:20:13.85288+00,5,2026-10-14 10:20:13.382713+00"]).unwrap();
        assert_eq!(info.dealloc, 5);
        assert_eq!(
            info.stats_reset,
            Some("2026-10-14T10:20:13.382713Z".parse().unwrap())
        );
    }

    #[test]
    fn info_without_stats_reset_is_none() {
        let info = read_info(&["2026-10-14 10:20:13+00,0,"]).unwrap();
        assert_eq!(info.stats_reset, None);
    }

    #[test]
    fn info_missing_dealloc_or_rows_is_an_error() {
        let err =
            Info::from_reader("snapshot_at,stats_reset\n".as_bytes(), "info.csv").unwrap_err();
        assert!(
            matches!(
                err,
                SnapshotError::MissingColumn {
                    column: "dealloc",
                    ..
                }
            ),
            "{err}"
        );
        let err = read_info(&[]).unwrap_err();
        assert!(matches!(err, SnapshotError::NoRows { .. }), "{err}");
    }

    #[test]
    fn with_info_attaches_the_info() {
        let info = Info {
            dealloc: 3,
            stats_reset: None,
        };
        let snap = read(&[ROW]).unwrap().with_info(info);
        assert_eq!(snap.info, Some(info));
    }

    #[test]
    fn missing_file_error_names_the_path() {
        let err = Snapshot::from_path(std::path::Path::new("/nonexistent/pgss.csv")).unwrap_err();
        assert!(matches!(err, SnapshotError::Io { .. }), "{err}");
        assert!(err.to_string().contains("/nonexistent/pgss.csv"), "{err}");
        let err = Info::from_path(std::path::Path::new("/nonexistent/info.csv")).unwrap_err();
        assert!(err.to_string().contains("/nonexistent/info.csv"), "{err}");
    }
}
