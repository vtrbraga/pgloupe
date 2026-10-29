//! Accumulators split by UTC hour, the unit the store keeps.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use super::{
    Merge,
    acc::{Accumulators, SharedQueries, add_maps},
};
use crate::log::{BlockPair, LogRecord};

/// Seconds in a slice.
pub const HOUR: i64 = 3_600;

/// The start of the UTC hour that contains `second`.
#[must_use]
pub fn hour_of(second: i64) -> i64 {
    second.div_euclid(HOUR) * HOUR
}

/// One [`Accumulators`] per UTC hour, each exactly what a run over that hour's records would give.
#[derive(Debug, Clone, PartialEq)]
pub struct Slices {
    limit: usize,
    bucket_secs: i64,
    slowest: usize,
    params: bool,
    /// Keyed by the hour's start, in Unix seconds.
    pub by_hour: BTreeMap<i64, Accumulators>,
    shared: SharedHours,
}

/// The fingerprints of each hour, kept once for all the workers that fold into their own
/// [`Slices`]. Two values are equal whatever they share.
#[derive(Debug, Clone, Default)]
pub struct SharedHours(Option<Arc<Mutex<HourQueries>>>);

type HourQueries = BTreeMap<i64, Arc<SharedQueries>>;

impl SharedHours {
    /// An empty set of hours.
    #[must_use]
    pub fn new() -> Self {
        SharedHours(Some(Arc::default()))
    }

    /// The shared fingerprints of the hour starting at `hour`.
    fn of(&self, hour: i64) -> Option<Arc<SharedQueries>> {
        let hours = self.0.as_ref()?;
        let mut hours = hours
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Some(Arc::clone(
            hours.entry(hour).or_insert_with(SharedQueries::new),
        ))
    }
}

impl PartialEq for SharedHours {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl Slices {
    /// `limit` bounds the fingerprints kept per hour; `bucket_secs` is the timeline resolution.
    #[must_use]
    pub fn new(limit: usize, bucket_secs: i64) -> Self {
        Slices {
            limit,
            bucket_secs,
            slowest: super::slowest::DEFAULT_KEPT,
            params: false,
            by_hour: BTreeMap::new(),
            shared: SharedHours::default(),
        }
    }

    /// Folds the statements of every hour into `shared`, so several `Slices` keep each fingerprint
    /// once between them. Call [`Slices::absorb_shared`] on the merged result.
    #[must_use]
    pub fn with_shared(mut self, shared: SharedHours) -> Self {
        self.shared = shared;
        self
    }

    /// Moves the shared fingerprints into the hours' accumulators.
    pub fn absorb_shared(&mut self) {
        for acc in self.by_hour.values_mut() {
            acc.absorb_shared();
        }
    }

    /// A new accumulator for the hour starting at `hour`.
    fn new_hour(&self, hour: i64) -> Accumulators {
        let acc =
            Accumulators::new(self.limit, self.bucket_secs).with_capture(self.slowest, self.params);
        match self.shared.of(hour) {
            Some(queries) => acc.with_shared(queries),
            None => acc,
        }
    }

    /// Keeps the `slowest` slowest executions per hour, with bind parameters when `params` is set.
    #[must_use]
    pub fn with_capture(mut self, slowest: usize, params: bool) -> Self {
        self.slowest = slowest;
        self.params = params;
        self
    }

    /// Folds `record` into its hour.
    pub fn add_record(&mut self, record: &LogRecord) {
        let hour = hour_of(record.ts.as_second());
        if !self.by_hour.contains_key(&hour) {
            let acc = self.new_hour(hour);
            self.by_hour.insert(hour, acc);
        }
        if let Some(acc) = self.by_hour.get_mut(&hour) {
            acc.add_record(record);
        }
    }

    /// Folds lock waits into the hour they were logged in.
    pub fn add_blocks(&mut self, blocks: &[BlockPair]) {
        for block in blocks {
            let hour = hour_of(block.at.as_second());
            if !self.by_hour.contains_key(&hour) {
                let acc = self.new_hour(hour);
                self.by_hour.insert(hour, acc);
            }
            if let Some(acc) = self.by_hour.get_mut(&hour) {
                acc.add_blocks(std::slice::from_ref(block));
            }
        }
    }

    /// Lowers every hour's fingerprint limit.
    pub fn recap(&mut self, limit: usize) {
        self.limit = limit;
        for acc in self.by_hour.values_mut() {
            acc.recap(limit);
        }
    }
}

impl Merge for Slices {
    fn merge(&mut self, other: Self) {
        self.limit = self.limit.max(other.limit);
        add_maps(&mut self.by_hour, other.by_hour);
    }
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;

    use super::*;
    use crate::log::Severity;

    fn spill(second: i64, bytes: u64, app: &str) -> LogRecord {
        let mut record = LogRecord::new(
            Timestamp::from_second(1_760_000_000 + second).unwrap(),
            Severity::Log,
            format!("temporary file: path \"p\", size {bytes}"),
        );
        record.statement = Some("SELECT 1 FROM big".to_owned());
        record.application = Some(app.to_owned());
        record
    }

    fn stored(slices: &Slices) -> Accumulators {
        let (_, acc) = slices.by_hour.iter().next().unwrap();
        serde_json::from_str(&serde_json::to_string(acc).unwrap()).unwrap()
    }

    #[test]
    fn stored_hours_add_up_the_same_spillers_as_one_run() {
        let mut first = Slices::new(100, 60);
        for n in 0..5_u32 {
            first.add_record(&spill(i64::from(n), 100, &format!("app{n}")));
        }
        first.add_record(&spill(6, 50, "late"));
        let mut second = Slices::new(100, 60);
        second.add_record(&spill(7, 500, "late"));
        first.recap(100);
        second.recap(100);
        let mut merged = stored(&first);
        merged.merge(stored(&second));
        merged.recap(100);
        let query = merged.queries.values().next().unwrap();
        let late = query
            .spilled_by
            .iter()
            .find(|(key, _)| merged.who[key].application.as_deref() == Some("late"))
            .map(|(_, spill)| spill.bytes);
        assert_eq!(late, Some(550));
    }
}
