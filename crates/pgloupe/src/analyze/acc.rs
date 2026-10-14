//! Mergeable accumulators: per-fingerprint statistics and global totals.

use std::collections::BTreeMap;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::{
    Merge,
    dimensions::{self, Attributes, Dimensions, Tally},
    event::{Event, StatementKind, classify},
    fingerprint::{display_text, fingerprint, sql_hash, statement_kind},
    kind::QueryKind,
    sketch::Sketch,
};
use crate::log::LogRecord;

/// Milliseconds to whole microseconds (sums of integers are exact and order independent).
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to non-negative and far below u64::MAX"
)]
pub fn to_micros(ms: f64) -> u64 {
    if ms.is_finite() && ms > 0.0 {
        (ms * 1_000.0).round() as u64
    } else {
        0
    }
}

fn earliest(a: Option<Timestamp>, b: Option<Timestamp>) -> Option<Timestamp> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

fn latest(a: Option<Timestamp>, b: Option<Timestamp>) -> Option<Timestamp> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

pub(super) fn add_maps<K: Ord, V: Merge>(into: &mut BTreeMap<K, V>, from: BTreeMap<K, V>) {
    for (key, value) in from {
        match into.entry(key) {
            std::collections::btree_map::Entry::Occupied(mut slot) => slot.get_mut().merge(value),
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(value);
            }
        }
    }
}

/// Maps with non-string keys travel as a list of `[key, value]` pairs.
mod pairs {
    use std::collections::BTreeMap;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<K: Serialize, V: Serialize, S: Serializer>(
        map: &BTreeMap<K, V>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(map.iter())
    }

    pub fn deserialize<'de, K, V, D>(deserializer: D) -> Result<BTreeMap<K, V>, D::Error>
    where
        K: Deserialize<'de> + Ord,
        V: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        Ok(Vec::<(K, V)>::deserialize(deserializer)?
            .into_iter()
            .collect())
    }
}

/// Statistics of one normalized statement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryStats {
    pub id: u64,
    pub parsed: bool,
    /// What the statement does.
    pub kind: QueryKind,
    /// Normalized text of the representative sample (see `sample`).
    pub text: String,
    /// The `query_id` of the representative sample.
    pub query_id: Option<i64>,
    /// `(timestamp, hash of the raw text, query_id)` of the earliest sample seen; it decides `text` and
    /// `query_id`, which makes both independent of arrival order and of how records are split.
    sample: Option<(Timestamp, u64, Option<i64>)>,
    /// `Statement` and `Execute` events.
    pub calls: u64,
    /// `calls` made with the simple protocol (`statement:`).
    pub statement_calls: u64,
    /// `calls` made with the extended protocol (`execute`).
    pub execute_calls: u64,
    /// Calls that carried a duration.
    pub timed: u64,
    pub total_us: u64,
    pub min_us: Option<u64>,
    pub max_us: u64,
    pub sketch: Sketch,
    pub first_seen: Option<Timestamp>,
    pub last_seen: Option<Timestamp>,
    pub errors: u64,
    pub lock_waits: u64,
    pub lock_wait_us: u64,
    pub temp_files: u64,
    pub temp_bytes: u64,
}

impl QueryStats {
    fn new(id: u64, parsed: bool, text: String, kind: QueryKind) -> Self {
        QueryStats {
            id,
            parsed,
            kind,
            text,
            query_id: None,
            sample: None,
            calls: 0,
            statement_calls: 0,
            execute_calls: 0,
            timed: 0,
            total_us: 0,
            min_us: None,
            max_us: 0,
            sketch: Sketch::new(),
            first_seen: None,
            last_seen: None,
            errors: 0,
            lock_waits: 0,
            lock_wait_us: 0,
            temp_files: 0,
            temp_bytes: 0,
        }
    }

    /// Offers a spelling of this statement as the representative sample: the one with the smallest
    /// `(timestamp, hash)` wins, wherever and whenever it is seen.
    fn offer(&mut self, ts: Timestamp, sql: &str, query_id: Option<i64>) {
        let key = (ts, sql_hash(sql), query_id);
        if self.sample.is_none_or(|current| key < current) {
            self.sample = Some(key);
            self.text = display_text(sql, self.parsed);
            self.query_id = query_id;
        }
    }

    /// Mean duration of the timed calls, in milliseconds.
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "microsecond sums stay far below 2^53"
    )]
    pub fn mean_ms(&self) -> Option<f64> {
        (self.timed > 0).then(|| self.total_us as f64 / self.timed as f64 / 1_000.0)
    }

    fn seen(&mut self, ts: Timestamp) {
        self.first_seen = earliest(self.first_seen, Some(ts));
        self.last_seen = latest(self.last_seen, Some(ts));
    }

    fn call(&mut self, duration_ms: Option<f64>, ts: Timestamp, kind: StatementKind) {
        self.calls += 1;
        match kind {
            StatementKind::Execute => self.execute_calls += 1,
            _ => self.statement_calls += 1,
        }
        self.seen(ts);
        if let Some(ms) = duration_ms {
            let us = to_micros(ms);
            self.timed += 1;
            self.total_us += us;
            self.min_us = Some(self.min_us.map_or(us, |m| m.min(us)));
            self.max_us = self.max_us.max(us);
            self.sketch.add(ms);
        }
    }
}

impl Merge for QueryStats {
    fn merge(&mut self, other: Self) {
        if other
            .sample
            .is_some_and(|theirs| self.sample.is_none_or(|ours| theirs < ours))
        {
            self.sample = other.sample;
            self.text = other.text;
            self.query_id = other.query_id;
        }
        self.parsed &= other.parsed;
        if self.kind == QueryKind::Other {
            self.kind = other.kind;
        }
        self.calls += other.calls;
        self.statement_calls += other.statement_calls;
        self.execute_calls += other.execute_calls;
        self.timed += other.timed;
        self.total_us += other.total_us;
        self.min_us = match (self.min_us, other.min_us) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        self.max_us = self.max_us.max(other.max_us);
        self.sketch.merge(other.sketch);
        self.first_seen = earliest(self.first_seen, other.first_seen);
        self.last_seen = latest(self.last_seen, other.last_seen);
        self.errors += other.errors;
        self.lock_waits += other.lock_waits;
        self.lock_wait_us += other.lock_wait_us;
        self.temp_files += other.temp_files;
        self.temp_bytes += other.temp_bytes;
    }
}

/// One kind of error: `(level, sqlstate)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorStat {
    pub count: u64,
    pub first_message: String,
    pub first_seen: Timestamp,
    pub last_seen: Timestamp,
}

impl Merge for ErrorStat {
    fn merge(&mut self, other: Self) {
        self.count += other.count;
        if other.first_seen < self.first_seen
            || (other.first_seen == self.first_seen && other.first_message < self.first_message)
        {
            self.first_message = other.first_message;
        }
        self.first_seen = self.first_seen.min(other.first_seen);
        self.last_seen = self.last_seen.max(other.last_seen);
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LockStats {
    pub waits: u64,
    pub acquired: u64,
    /// Sum of the "acquired after X ms" waits.
    pub wait_total_us: u64,
    pub wait_max_us: u64,
    pub deadlocks: u64,
}

impl Merge for LockStats {
    fn merge(&mut self, other: Self) {
        self.waits += other.waits;
        self.acquired += other.acquired;
        self.wait_total_us += other.wait_total_us;
        self.wait_max_us = self.wait_max_us.max(other.wait_max_us);
        self.deadlocks += other.deadlocks;
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TempStats {
    pub files: u64,
    pub bytes: u64,
    pub max_bytes: u64,
}

impl Merge for TempStats {
    fn merge(&mut self, other: Self) {
        self.files += other.files;
        self.bytes += other.bytes;
        self.max_bytes = self.max_bytes.max(other.max_bytes);
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ConnStats {
    pub received: u64,
    pub authenticated: u64,
    pub authorized: u64,
    pub disconnections: u64,
    /// Session durations in milliseconds.
    pub session: Sketch,
}

impl Merge for ConnStats {
    fn merge(&mut self, other: Self) {
        self.received += other.received;
        self.authenticated += other.authenticated;
        self.authorized += other.authorized;
        self.disconnections += other.disconnections;
        self.session.merge(other.session);
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CheckpointStats {
    pub started: u64,
    pub done: u64,
    /// "starting" lines by reason.
    pub reasons: BTreeMap<String, u64>,
    pub buffers: u64,
    pub write: Sketch,
    pub sync: Sketch,
    pub total: Sketch,
}

impl Merge for CheckpointStats {
    fn merge(&mut self, other: Self) {
        self.started += other.started;
        self.done += other.done;
        for (reason, n) in other.reasons {
            *self.reasons.entry(reason).or_insert(0) += n;
        }
        self.buffers += other.buffers;
        self.write.merge(other.write);
        self.sync.merge(other.sync);
        self.total.merge(other.total);
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TableStats {
    pub vacuums: u64,
    pub analyzes: u64,
    pub total_us: u64,
    pub max_us: u64,
}

impl Merge for TableStats {
    fn merge(&mut self, other: Self) {
        self.vacuums += other.vacuums;
        self.analyzes += other.analyzes;
        self.total_us += other.total_us;
        self.max_us = self.max_us.max(other.max_us);
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Bucket {
    pub statements: u64,
    pub errors: u64,
    pub connections: u64,
    pub lock_waits: u64,
}

/// Event counts per time bucket (`bucket_secs` wide).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Timeline {
    pub bucket_secs: i64,
    /// Keyed by the bucket's start, in Unix seconds.
    pub buckets: BTreeMap<i64, Bucket>,
}

impl Timeline {
    fn add(&mut self, ts: Timestamp, update: impl FnOnce(&mut Bucket)) {
        let start = ts.as_second().div_euclid(self.bucket_secs) * self.bucket_secs;
        update(self.buckets.entry(start).or_default());
    }
}

impl Timeline {
    /// The same counts in buckets of `bucket_secs` (a multiple of the current width).
    #[must_use]
    pub fn rebucket(self, bucket_secs: i64) -> Timeline {
        let width = bucket_secs.max(1);
        let mut wider = Timeline {
            bucket_secs: width,
            buckets: BTreeMap::new(),
        };
        for (start, bucket) in self.buckets {
            let into = wider
                .buckets
                .entry(start.div_euclid(width) * width)
                .or_default();
            into.statements += bucket.statements;
            into.errors += bucket.errors;
            into.connections += bucket.connections;
            into.lock_waits += bucket.lock_waits;
        }
        wider
    }
}

impl Merge for Timeline {
    fn merge(&mut self, other: Self) {
        for (start, bucket) in other.buckets {
            let into = self.buckets.entry(start).or_default();
            into.statements += bucket.statements;
            into.errors += bucket.errors;
            into.connections += bucket.connections;
            into.lock_waits += bucket.lock_waits;
        }
    }
}

/// Everything that is not per fingerprint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Totals {
    pub records: u64,
    pub first: Option<Timestamp>,
    pub last: Option<Timestamp>,
    /// Records per [`Event::kind`].
    pub events: BTreeMap<String, u64>,
    /// Records per severity label.
    pub levels: BTreeMap<String, u64>,
    /// Records at `ERROR` or above, by `(level, sqlstate)`.
    #[serde(with = "pairs")]
    pub errors: BTreeMap<(String, Option<String>), ErrorStat>,
    pub locks: LockStats,
    pub temp: TempStats,
    pub connections: ConnStats,
    /// Keyed by `restart` (false = checkpoint, true = restartpoint).
    #[serde(with = "pairs")]
    pub checkpoints: BTreeMap<bool, CheckpointStats>,
    pub autovacuum: BTreeMap<String, TableStats>,
    pub timeline: Timeline,
    pub unparsed_statements: u64,
    /// Durations of every statement that carried one, in milliseconds.
    pub durations: Sketch,
}

impl Totals {
    fn new(bucket_secs: i64) -> Self {
        Totals {
            records: 0,
            first: None,
            last: None,
            events: BTreeMap::new(),
            levels: BTreeMap::new(),
            errors: BTreeMap::new(),
            locks: LockStats::default(),
            temp: TempStats::default(),
            connections: ConnStats::default(),
            checkpoints: BTreeMap::new(),
            autovacuum: BTreeMap::new(),
            timeline: Timeline {
                bucket_secs: bucket_secs.max(1),
                buckets: BTreeMap::new(),
            },
            unparsed_statements: 0,
            durations: Sketch::new(),
        }
    }

    fn add(&mut self, record: &LogRecord, event: &Event) {
        let ts = record.ts;
        self.records += 1;
        self.first = earliest(self.first, Some(ts));
        self.last = latest(self.last, Some(ts));
        *self.events.entry(event.kind().to_owned()).or_insert(0) += 1;
        *self
            .levels
            .entry(record.level.label().to_owned())
            .or_insert(0) += 1;
        if record.level.is_error() {
            self.add_error(record);
        }
        match event {
            Event::Statement {
                kind: StatementKind::Statement | StatementKind::Execute,
                duration_ms,
                ..
            } => {
                self.timeline.add(ts, |b| b.statements += 1);
                if let Some(ms) = duration_ms {
                    self.durations.add(*ms);
                }
            }
            Event::Duration { duration_ms } => self.durations.add(*duration_ms),
            Event::LockWait { .. } => {
                self.locks.waits += 1;
                self.timeline.add(ts, |b| b.lock_waits += 1);
            }
            Event::LockAcquired { waited_ms, .. } => {
                let us = to_micros(*waited_ms);
                self.locks.acquired += 1;
                self.locks.wait_total_us += us;
                self.locks.wait_max_us = self.locks.wait_max_us.max(us);
            }
            Event::Deadlock => self.locks.deadlocks += 1,
            Event::TempFile { bytes, .. } => {
                self.temp.files += 1;
                self.temp.bytes += bytes;
                self.temp.max_bytes = self.temp.max_bytes.max(*bytes);
            }
            Event::CheckpointStart { restart, reason } => {
                let stats = self.checkpoints.entry(*restart).or_default();
                stats.started += 1;
                *stats.reasons.entry(reason.clone()).or_insert(0) += 1;
            }
            Event::CheckpointDone {
                restart,
                buffers,
                write_ms,
                sync_ms,
                total_ms,
            } => {
                let stats = self.checkpoints.entry(*restart).or_default();
                stats.done += 1;
                stats.buffers += buffers;
                stats.write.add(*write_ms);
                stats.sync.add(*sync_ms);
                stats.total.add(*total_ms);
            }
            Event::Autovacuum {
                analyze,
                table,
                elapsed_ms,
            } => {
                let stats = self.autovacuum.entry(table.clone()).or_default();
                if *analyze {
                    stats.analyzes += 1;
                } else {
                    stats.vacuums += 1;
                }
                let us = elapsed_ms.map_or(0, to_micros);
                stats.total_us += us;
                stats.max_us = stats.max_us.max(us);
            }
            Event::ConnectionReceived { .. } => self.connections.received += 1,
            Event::ConnectionAuthenticated => self.connections.authenticated += 1,
            Event::ConnectionAuthorized => {
                self.connections.authorized += 1;
                self.timeline.add(ts, |b| b.connections += 1);
            }
            Event::Disconnection { session_ms } => {
                self.connections.disconnections += 1;
                self.connections.session.add(*session_ms);
            }
            _ => {}
        }
    }

    fn add_error(&mut self, record: &LogRecord) {
        let ts = record.ts;
        self.timeline.add(ts, |b| b.errors += 1);
        let key = (record.level.label().to_owned(), record.sqlstate.clone());
        let stat = self.errors.entry(key).or_insert_with(|| ErrorStat {
            count: 0,
            first_message: record.message.clone(),
            first_seen: ts,
            last_seen: ts,
        });
        stat.count += 1;
        if ts < stat.first_seen || (ts == stat.first_seen && record.message < stat.first_message) {
            stat.first_seen = ts;
            stat.first_message.clone_from(&record.message);
        }
        stat.last_seen = stat.last_seen.max(ts);
    }
}

impl Merge for Totals {
    fn merge(&mut self, other: Self) {
        self.records += other.records;
        self.first = earliest(self.first, other.first);
        self.last = latest(self.last, other.last);
        for (kind, n) in other.events {
            *self.events.entry(kind).or_insert(0) += n;
        }
        for (level, n) in other.levels {
            *self.levels.entry(level).or_insert(0) += n;
        }
        add_maps(&mut self.errors, other.errors);
        self.locks.merge(other.locks);
        self.temp.merge(other.temp);
        self.connections.merge(other.connections);
        add_maps(&mut self.checkpoints, other.checkpoints);
        add_maps(&mut self.autovacuum, other.autovacuum);
        self.timeline.merge(other.timeline);
        self.unparsed_statements += other.unparsed_statements;
        self.durations.merge(other.durations);
    }
}

/// All statistics of a run (or a part of one).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Accumulators {
    pub totals: Totals,
    pub queries: BTreeMap<u64, QueryStats>,
    /// Statistics of fingerprints beyond the limit.
    pub overflow: QueryStats,
    /// Statements, errors and sessions by database, user, application and host.
    pub dims: Dimensions,
    limit: usize,
}

impl Accumulators {
    /// `limit` bounds the distinct fingerprints kept; `bucket_secs` is the timeline resolution.
    #[must_use]
    pub fn new(limit: usize, bucket_secs: i64) -> Self {
        Accumulators {
            totals: Totals::new(bucket_secs),
            queries: BTreeMap::new(),
            overflow: QueryStats::new(0, true, "<overflow>".to_owned(), QueryKind::Other),
            dims: Dimensions::default(),
            limit: limit.max(1),
        }
    }

    /// Classifies `record` and folds it in.
    pub fn add_record(&mut self, record: &LogRecord) {
        let event = classify(record);
        self.totals.add(record, &event);
        self.tally(record, &event);
        let ts = record.ts;
        match &event {
            Event::Statement {
                kind: kind @ (StatementKind::Statement | StatementKind::Execute),
                duration_ms,
                text,
                ..
            } => {
                let query = self.query_for(text, ts, record.query_id);
                query.call(*duration_ms, ts, *kind);
            }
            Event::LockWait { .. } => {
                if let Some(sql) = record.statement.as_deref() {
                    let query = self.query_for(sql, ts, record.query_id);
                    query.seen(ts);
                    query.lock_waits += 1;
                }
            }
            Event::LockAcquired { waited_ms, .. } => {
                if let Some(sql) = record.statement.as_deref() {
                    let query = self.query_for(sql, ts, record.query_id);
                    query.seen(ts);
                    query.lock_wait_us += to_micros(*waited_ms);
                }
            }
            Event::TempFile { bytes, .. } => {
                if let Some(sql) = record.statement.as_deref() {
                    let query = self.query_for(sql, ts, record.query_id);
                    query.seen(ts);
                    query.temp_files += 1;
                    query.temp_bytes += bytes;
                }
            }
            _ => {}
        }
        if record.level.is_error()
            && let Some(sql) = record.statement.as_deref()
        {
            let query = self.query_for(sql, ts, record.query_id);
            query.seen(ts);
            query.errors += 1;
        }
    }

    /// Counts the record under its database, user, application and host.
    fn tally(&mut self, record: &LogRecord, event: &Event) {
        let attributes = Attributes {
            database: record.database.as_deref(),
            user: record.user.as_deref(),
            application: record.application.as_deref(),
            host: record.host.as_deref(),
        };
        if record.level.is_error() {
            self.dims.add(&attributes, Tally::Error);
        }
        match event {
            Event::Statement {
                kind: StatementKind::Statement | StatementKind::Execute,
                duration_ms,
                ..
            } => self.dims.add(&attributes, Tally::Statement(*duration_ms)),
            Event::ConnectionAuthorized => self.dims.add(&attributes, Tally::Connection),
            Event::Disconnection { session_ms } => {
                self.dims.add(&attributes, Tally::Session(*session_ms));
            }
            _ => {}
        }
    }

    /// The statistics of `sql`'s fingerprint, creating them (or using the overflow bucket). The
    /// statement is offered as the representative sample of its fingerprint.
    fn query_for(&mut self, sql: &str, ts: Timestamp, query_id: Option<i64>) -> &mut QueryStats {
        let found = fingerprint(sql);
        if !found.parsed {
            self.totals.unparsed_statements += 1;
        }
        if !self.queries.contains_key(&found.id) && self.queries.len() >= self.limit {
            return &mut self.overflow;
        }
        let query = self.queries.entry(found.id).or_insert_with(|| {
            QueryStats::new(found.id, found.parsed, String::new(), statement_kind(sql))
        });
        query.offer(ts, sql, query_id);
        query
    }

    /// Lowers the limit to `limit`, folding the least called fingerprints into the overflow bucket
    /// (ties: the larger id goes first), so the result does not depend on arrival order.
    pub fn recap(&mut self, limit: usize) {
        self.dims.recap(dimensions::LIMIT);
        self.limit = limit.max(1);
        if self.queries.len() <= self.limit {
            return;
        }
        let mut ranked: Vec<(u64, u64)> =
            self.queries.iter().map(|(id, q)| (*id, q.calls)).collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        for (id, _) in ranked.split_off(self.limit) {
            if let Some(mut query) = self.queries.remove(&id) {
                // The overflow bucket keeps its own identity: only counters flow into it.
                query.text = String::new();
                query.query_id = None;
                query.sample = None;
                query.parsed = true;
                self.overflow.merge(query);
            }
        }
    }
}

impl Merge for Accumulators {
    fn merge(&mut self, other: Self) {
        self.totals.merge(other.totals);
        add_maps(&mut self.queries, other.queries);
        self.overflow.merge(other.overflow);
        self.dims.merge(other.dims);
        let limit = self.limit.max(other.limit);
        self.recap(limit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{log::Severity, testutil::approx_eq};

    /// A multiple of 60, so one-minute buckets start at second 0.
    fn at(second: i64) -> Timestamp {
        Timestamp::from_second(1_759_999_980 + second).unwrap()
    }

    fn log(second: i64, message: &str) -> LogRecord {
        LogRecord::new(at(second), Severity::Log, message)
    }

    fn statement(second: i64, ms: f64, sql: &str) -> LogRecord {
        log(second, &format!("duration: {ms} ms  statement: {sql}"))
    }

    fn acc() -> Accumulators {
        Accumulators::new(1_000, 60)
    }

    #[test]
    fn rebucket_sums_minutes_into_the_wider_bucket() {
        let bucket = |statements| Bucket {
            statements,
            errors: 1,
            connections: 0,
            lock_waits: 0,
        };
        let minutes = Timeline {
            bucket_secs: 60,
            buckets: BTreeMap::from([(0, bucket(1)), (60, bucket(2)), (3_600, bucket(4))]),
        };
        let hours = minutes.clone().rebucket(3_600);
        assert_eq!(hours.bucket_secs, 3_600);
        assert_eq!(
            hours.buckets,
            BTreeMap::from([
                (
                    0,
                    Bucket {
                        errors: 2,
                        ..bucket(3)
                    }
                ),
                (3_600, bucket(4))
            ])
        );
        assert_eq!(minutes.clone().rebucket(60), minutes);
    }

    #[test]
    fn statements_group_by_fingerprint_and_time() {
        let mut a = acc();
        a.add_record(&statement(0, 10.0, "SELECT * FROM film WHERE film_id = 1"));
        a.add_record(&statement(1, 30.0, "select * from film where film_id = 2"));
        a.add_record(&statement(2, 5.0, "SELECT * FROM actor"));
        assert_eq!(a.queries.len(), 2);
        let film = a
            .queries
            .values()
            .find(|q| q.text.contains("film"))
            .unwrap();
        assert_eq!(film.calls, 2);
        assert_eq!(film.timed, 2);
        assert_eq!(film.total_us, 40_000);
        assert_eq!(film.min_us, Some(10_000));
        assert_eq!(film.max_us, 30_000);
        assert!(approx_eq(film.mean_ms().unwrap(), 20.0));
        assert_eq!(film.text, "SELECT * FROM film WHERE film_id = $1");
        assert_eq!(film.first_seen, Some(at(0)));
        assert_eq!(film.last_seen, Some(at(1)));
        assert_eq!(film.sketch.count(), 2);
        assert_eq!(a.totals.events["statement"], 3);
        assert_eq!(a.totals.records, 3);
    }

    #[test]
    fn statements_without_durations_count_calls_only() {
        let mut a = acc();
        a.add_record(&log(0, "statement: SELECT 1"));
        let q = a.queries.values().next().unwrap();
        assert_eq!((q.calls, q.timed, q.total_us), (1, 0, 0));
        assert_eq!(q.min_us, None);
        assert_eq!(q.mean_ms(), None);
    }

    #[test]
    fn parse_and_bind_are_counted_in_totals_but_not_as_calls() {
        let mut a = acc();
        a.add_record(&log(0, "duration: 1 ms  parse S_1: SELECT $1"));
        a.add_record(&log(0, "duration: 1 ms  bind S_1: SELECT $1"));
        a.add_record(&log(0, "duration: 2 ms  execute S_1: SELECT $1"));
        assert_eq!(a.queries.values().map(|q| q.calls).sum::<u64>(), 1);
        assert_eq!(a.totals.events["parse"], 1);
        assert_eq!(a.totals.events["bind"], 1);
        assert_eq!(a.totals.events["execute"], 1);
    }

    #[test]
    fn unparsable_statements_are_counted() {
        let mut a = acc();
        a.add_record(&statement(0, 1.0, "SELEC 1"));
        assert_eq!(a.totals.unparsed_statements, 1);
        assert!(!a.queries.values().next().unwrap().parsed);
    }

    #[test]
    fn errors_locks_and_temp_files_are_attributed_to_the_statement() {
        let mut a = acc();
        let sql = "UPDATE film SET x = 1 WHERE film_id = 10";
        a.add_record(&statement(0, 1.0, sql));
        let mut err = LogRecord::new(at(1), Severity::Error, "boom");
        err.sqlstate = Some("23505".to_owned());
        err.statement = Some(sql.to_owned());
        a.add_record(&err);
        let mut wait = log(
            2,
            "process 9 still waiting for ShareLock on transaction 5 after 200.5 ms",
        );
        wait.statement = Some(sql.to_owned());
        a.add_record(&wait);
        let mut got = log(
            3,
            "process 9 acquired ShareLock on transaction 5 after 400.25 ms",
        );
        got.statement = Some(sql.to_owned());
        a.add_record(&got);
        let mut temp = log(4, "temporary file: path \"x\", size 1000");
        temp.statement = Some(sql.to_owned());
        a.add_record(&temp);
        let q = a.queries.values().next().unwrap();
        assert_eq!(q.calls, 1);
        assert_eq!(q.errors, 1);
        assert_eq!((q.lock_waits, q.lock_wait_us), (1, 400_250));
        assert_eq!((q.temp_files, q.temp_bytes), (1, 1_000));
        assert_eq!(
            a.totals.errors[&("ERROR".to_owned(), Some("23505".to_owned()))].count,
            1
        );
        assert_eq!(a.totals.locks.waits, 1);
        assert_eq!(a.totals.locks.acquired, 1);
        assert_eq!(a.totals.locks.wait_max_us, 400_250);
        assert_eq!(a.totals.temp.bytes, 1_000);
    }

    #[test]
    fn deadlocks_count_as_errors_too() {
        let mut a = acc();
        let mut r = LogRecord::new(at(0), Severity::Error, "deadlock detected");
        r.sqlstate = Some("40P01".to_owned());
        a.add_record(&r);
        assert_eq!(a.totals.locks.deadlocks, 1);
        assert_eq!(a.totals.events["deadlock"], 1);
        assert_eq!(
            a.totals.errors[&("ERROR".to_owned(), Some("40P01".to_owned()))].count,
            1
        );
    }

    #[test]
    fn connections_checkpoints_and_autovacuum() {
        let mut a = acc();
        a.add_record(&log(0, "connection received: host=1.2.3.4 port=1"));
        a.add_record(&log(0, "connection authorized: user=u database=d"));
        a.add_record(&log(
            1,
            "disconnection: session time: 0:00:01.500 user=u database=d host=h port=1",
        ));
        a.add_record(&log(2, "checkpoint starting: time"));
        a.add_record(&log(3, "checkpoint complete: wrote 3 buffers (0.0%); 0 WAL file(s) added, 0 removed, 0 recycled; write=0.003 s, sync=0.001 s, total=0.012 s; sync files=2, longest=0.001 s, average=0.001 s; distance=0 kB, estimate=0 kB"));
        a.add_record(&log(4, "automatic vacuum of table \"d.s.t\": index scans: 0\nsystem usage: CPU: user: 0.00 s, system: 0.00 s, elapsed: 0.25 s"));
        a.add_record(&log(5, "automatic analyze of table \"d.s.t\"\nsystem usage: CPU: user: 0.00 s, system: 0.00 s, elapsed: 0.10 s"));
        let t = &a.totals;
        assert_eq!(
            (
                t.connections.received,
                t.connections.authorized,
                t.connections.disconnections
            ),
            (1, 1, 1)
        );
        assert_eq!(t.connections.session.count(), 1);
        let cp = &t.checkpoints[&false];
        assert_eq!((cp.started, cp.done, cp.buffers), (1, 1, 3));
        assert_eq!(cp.reasons["time"], 1);
        assert_eq!(cp.total.count(), 1);
        let table = &t.autovacuum["d.s.t"];
        assert_eq!(
            (table.vacuums, table.analyzes, table.total_us, table.max_us),
            (1, 1, 350_000, 250_000)
        );
    }

    #[test]
    fn the_timeline_buckets_by_time() {
        let mut a = Accumulators::new(100, 60);
        a.add_record(&statement(0, 1.0, "SELECT 1"));
        a.add_record(&statement(59, 1.0, "SELECT 1"));
        a.add_record(&statement(61, 1.0, "SELECT 1"));
        a.add_record(&log(70, "connection authorized: user=u database=d"));
        assert_eq!(a.totals.timeline.buckets.len(), 2);
        let counts: Vec<(u64, u64)> = a
            .totals
            .timeline
            .buckets
            .values()
            .map(|b| (b.statements, b.connections))
            .collect();
        assert_eq!(counts, [(2, 0), (1, 1)]);
    }

    #[test]
    fn levels_and_window_are_tracked() {
        let mut a = acc();
        a.add_record(&log(10, "whatever"));
        a.add_record(&LogRecord::new(at(5), Severity::Warning, "careful"));
        assert_eq!(a.totals.levels["LOG"], 1);
        assert_eq!(a.totals.levels["WARNING"], 1);
        assert_eq!(a.totals.first, Some(at(5)));
        assert_eq!(a.totals.last, Some(at(10)));
        assert_eq!(a.totals.events["other"], 2);
    }

    #[test]
    fn cap_folds_the_rarest_into_overflow() {
        let mut a = Accumulators::new(2, 60);
        for _ in 0..3 {
            a.add_record(&statement(0, 1.0, "SELECT a FROM t1"));
        }
        for _ in 0..2 {
            a.add_record(&statement(0, 1.0, "SELECT a FROM t2"));
        }
        a.add_record(&statement(0, 1.0, "SELECT a FROM t3"));
        a.add_record(&statement(0, 1.0, "SELECT a FROM t4"));
        assert_eq!(a.queries.len(), 2);
        assert_eq!(a.overflow.calls, 2);
    }

    fn chunk(offset: i64) -> Vec<LogRecord> {
        (0..40)
            .map(|i| {
                let n = i64::from(i);
                match i % 5 {
                    0 => statement(
                        offset + n,
                        1.0 + f64::from(i % 7),
                        &format!("SELECT * FROM t{} WHERE id = {n}", n % 6),
                    ),
                    1 => log(offset + n, "connection authorized: user=u database=d"),
                    2 => log(
                        offset + n,
                        &format!("temporary file: path \"p{n}\", size {}", n * 100),
                    ),
                    3 => {
                        let mut e = LogRecord::new(at(offset + n), Severity::Error, "boom");
                        e.sqlstate = Some(format!("2300{}", n % 3));
                        e.statement = Some(format!("SELECT * FROM t{}", n % 6));
                        e
                    }
                    _ => log(
                        offset + n,
                        "disconnection: session time: 0:00:01.000 user=u database=d host=h port=1",
                    ),
                }
            })
            .collect()
    }

    fn accumulate(records: &[LogRecord]) -> Accumulators {
        let mut a = acc();
        for r in records {
            a.add_record(r);
        }
        a
    }

    #[test]
    fn merging_equals_accumulating_everything_in_any_split() {
        let all: Vec<LogRecord> = [chunk(0), chunk(100), chunk(50)].concat();
        let whole = accumulate(&all);
        for split in [0, 1, 7, 60, 119, 120] {
            let (left, right) = all.split_at(split);
            let mut merged = accumulate(left);
            merged.merge(accumulate(right));
            assert_eq!(merged, whole, "split at {split}");
            let mut reversed = accumulate(right);
            reversed.merge(accumulate(left));
            assert_eq!(reversed, whole, "reversed split at {split}");
        }
    }

    #[test]
    fn merge_is_associative() {
        let (a, b, c) = (
            accumulate(&chunk(0)),
            accumulate(&chunk(100)),
            accumulate(&chunk(200)),
        );
        let mut left = a.clone();
        left.merge(b.clone());
        left.merge(c.clone());
        let mut bc = b;
        bc.merge(c);
        let mut right = a;
        right.merge(bc);
        assert_eq!(left, right);
    }

    #[test]
    fn merging_applies_the_limit_deterministically() {
        let mut one = Accumulators::new(2, 60);
        let mut two = Accumulators::new(2, 60);
        for _ in 0..3 {
            one.add_record(&statement(0, 1.0, "SELECT a FROM t1"));
        }
        for _ in 0..3 {
            two.add_record(&statement(0, 1.0, "SELECT a FROM t2"));
        }
        for _ in 0..2 {
            one.add_record(&statement(0, 1.0, "SELECT a FROM t3"));
            two.add_record(&statement(0, 1.0, "SELECT a FROM t3"));
        }
        let mut ab = one.clone();
        ab.merge(two.clone());
        let mut ba = two;
        ba.merge(one);
        assert_eq!(ab, ba);
        assert_eq!(ab.queries.len(), 2);
        // t3 has 4 calls, t1 and t2 have 3 each: the larger id of the tie is folded.
        assert_eq!(ab.overflow.calls, 3);
    }

    #[test]
    fn to_micros_rounds_and_clamps() {
        assert_eq!(to_micros(12.3456), 12_346);
        assert_eq!(to_micros(-1.0), 0);
        assert_eq!(to_micros(f64::NAN), 0);
        assert_eq!(to_micros(0.0004), 0);
    }

    fn spelled(second: i64, sql: &str, query_id: i64) -> LogRecord {
        let mut r = statement(second, 1.0, sql);
        r.query_id = Some(query_id);
        r
    }

    #[test]
    fn the_representative_text_and_query_id_do_not_depend_on_arrival_order() {
        // Two spellings of one fingerprint (the alias does not matter), one tied in time.
        let records = vec![
            spelled(0, "SELECT 1 AS two_lines", 20),
            spelled(0, "SELECT 1", 10),
            spelled(5, "SELECT 2", 30),
        ];
        let whole = accumulate(&records);
        let mut reversed: Vec<LogRecord> = records.clone();
        reversed.reverse();
        assert_eq!(accumulate(&reversed), whole);
        for split in 0..=records.len() {
            let (left, right) = records.split_at(split);
            let mut merged = accumulate(right);
            merged.merge(accumulate(left));
            assert_eq!(merged, whole, "split {split}");
        }
        let query = whole.queries.values().next().unwrap();
        assert_eq!(whole.queries.len(), 1);
        assert_eq!(query.text, "SELECT $1");
        assert_eq!(
            query.query_id,
            Some(10),
            "the sample that wins the tie supplies both"
        );
    }

    #[test]
    fn the_first_error_message_does_not_depend_on_arrival_order() {
        let mut first = LogRecord::new(at(0), Severity::Error, "zzz");
        first.sqlstate = Some("23505".to_owned());
        let mut second = LogRecord::new(at(0), Severity::Error, "aaa");
        second.sqlstate = Some("23505".to_owned());
        let both = accumulate(&[first.clone(), second.clone()]);
        let swapped = accumulate(&[second.clone(), first.clone()]);
        let mut merged = accumulate(&[first]);
        merged.merge(accumulate(&[second]));
        assert_eq!(both, swapped);
        assert_eq!(both, merged);
        let stat = &both.totals.errors[&("ERROR".to_owned(), Some("23505".to_owned()))];
        assert_eq!(stat.first_message, "aaa");
    }

    #[test]
    fn the_overflow_bucket_keeps_its_own_identity() {
        let mut a = Accumulators::new(1, 60);
        a.add_record(&spelled(0, "SELECT 1", 7));
        a.add_record(&spelled(0, "SELECT 1", 7));
        a.add_record(&spelled(0, "(SELECT 2) UNION (SELECT 3)", 8));
        a.recap(1);
        assert_eq!(a.overflow.text, "<overflow>");
        assert_eq!(a.overflow.query_id, None);
        assert!(a.overflow.parsed);
        assert_eq!(a.overflow.calls, 1);
    }

    #[test]
    fn the_query_id_of_a_timestamp_and_text_tie_does_not_depend_on_arrival_order() {
        let a = spelled(0, "SELECT 1", 111);
        let b = spelled(0, "SELECT 1", 222);
        let ab = accumulate(&[a.clone(), b.clone()]);
        let ba = accumulate(&[b.clone(), a.clone()]);
        let mut merged = accumulate(&[b]);
        merged.merge(accumulate(&[a]));
        assert_eq!(ab, ba);
        assert_eq!(ab, merged);
        assert_eq!(ab.queries.values().next().unwrap().query_id, Some(111));
    }

    #[test]
    fn statement_and_execute_calls_are_counted_separately() {
        let mut a = acc();
        a.add_record(&log(0, "duration: 1 ms  statement: SELECT 1"));
        a.add_record(&log(1, "duration: 1 ms  execute S_1: SELECT 1"));
        a.add_record(&log(2, "duration: 1 ms  execute S_1: SELECT 2"));
        let q = a.queries.values().next().unwrap();
        assert_eq!((q.calls, q.statement_calls, q.execute_calls), (3, 1, 2));
    }

    fn by(record: LogRecord, database: &str, user: &str, host: &str) -> LogRecord {
        LogRecord {
            database: Some(database.to_owned()),
            user: Some(user.to_owned()),
            host: Some(host.to_owned()),
            ..record
        }
    }

    #[test]
    fn records_are_counted_by_database_user_and_host() {
        let mut a = acc();
        a.add_record(&by(
            statement(1, 2.0, "SELECT 1"),
            "shop",
            "app",
            "10.0.0.1",
        ));
        a.add_record(&by(
            statement(2, 4.0, "SELECT 2"),
            "shop",
            "etl",
            "10.0.0.2",
        ));
        a.add_record(&by(
            LogRecord::new(at(3), Severity::Error, "boom"),
            "crm",
            "app",
            "10.0.0.1",
        ));
        a.add_record(&by(
            log(
                4,
                "disconnection: session time: 0:00:02.500 user=app database=shop host=10.0.0.1",
            ),
            "shop",
            "app",
            "10.0.0.1",
        ));
        assert_eq!(a.dims.databases["shop"].statements, 2);
        assert_eq!(a.dims.databases["shop"].total_us, 6_000);
        assert_eq!(a.dims.databases["crm"].errors, 1);
        assert_eq!(a.dims.users["app"].statements, 1);
        assert_eq!(a.dims.users["app"].sessions, 1);
        assert_eq!(a.dims.users["app"].session_us, 2_500_000);
        assert_eq!(a.dims.hosts["10.0.0.2"].statements, 1);
        assert_eq!(a.dims.applications[dimensions::NONE].statements, 2);
    }

    #[test]
    fn merged_accumulators_add_their_breakdowns() {
        let mut left = acc();
        let mut right = acc();
        left.add_record(&by(statement(1, 1.0, "SELECT 1"), "shop", "app", "h"));
        right.add_record(&by(statement(2, 1.0, "SELECT 1"), "shop", "app", "h"));
        left.merge(right);
        assert_eq!(left.dims.databases["shop"].statements, 2);
    }

    #[test]
    fn statement_durations_feed_the_global_sketch() {
        let mut a = acc();
        a.add_record(&statement(1, 3.0, "SELECT 1"));
        a.add_record(&statement(2, 30.0, "SELECT 2"));
        a.add_record(&log(3, "duration: 300.000 ms"));
        a.add_record(&log(4, "statement: SELECT 3"));
        assert_eq!(a.totals.durations.count(), 3);
        assert!(a.totals.durations.max().unwrap() >= 299.0);
    }

    #[test]
    fn each_fingerprint_knows_what_kind_of_statement_it_is() {
        let mut a = acc();
        a.add_record(&statement(1, 1.0, "SELECT 1"));
        a.add_record(&statement(2, 1.0, "INSERT INTO t VALUES (1)"));
        a.add_record(&statement(3, 1.0, "CREATE TABLE x (a int)"));
        let kinds: Vec<QueryKind> = a.queries.values().map(|q| q.kind).collect();
        for kind in [QueryKind::Select, QueryKind::Insert, QueryKind::Ddl] {
            assert!(kinds.contains(&kind), "{kinds:?}");
        }
        assert_eq!(a.overflow.kind, QueryKind::Other);
    }
}
