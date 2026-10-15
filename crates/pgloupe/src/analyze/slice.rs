//! Accumulators split by UTC hour, the unit the store keeps.

use std::collections::BTreeMap;

use super::{
    Merge,
    acc::{Accumulators, add_maps},
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
        let (limit, bucket_secs, slowest, params) =
            (self.limit, self.bucket_secs, self.slowest, self.params);
        self.by_hour
            .entry(hour_of(record.ts.as_second()))
            .or_insert_with(|| Accumulators::new(limit, bucket_secs).with_capture(slowest, params))
            .add_record(record);
    }

    /// Folds lock waits into the hour they were logged in.
    pub fn add_blocks(&mut self, blocks: &[BlockPair]) {
        let (limit, bucket_secs, slowest, params) =
            (self.limit, self.bucket_secs, self.slowest, self.params);
        for block in blocks {
            self.by_hour
                .entry(hour_of(block.at.as_second()))
                .or_insert_with(|| {
                    Accumulators::new(limit, bucket_secs).with_capture(slowest, params)
                })
                .add_blocks(std::slice::from_ref(block));
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
