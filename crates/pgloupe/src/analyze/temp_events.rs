//! The largest temp files of a run, kept as a bounded, mergeable list.

use std::cmp::Ordering;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::Merge;

/// Temp files kept unless the caller asks for another number.
pub const DEFAULT_KEPT: usize = 200;

/// One temp file, as its log record states it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TempEvent {
    /// When the log recorded it.
    pub ts: Timestamp,
    /// Its size in bytes.
    pub bytes: u64,
    /// The backend that logged it, when the prefix carries it.
    pub pid: Option<u32>,
    /// The [`super::slowest::who_key`] of the record.
    pub who: u64,
    /// The fingerprint of the statement the log names for it; 0 when the log names none.
    pub fingerprint: u64,
}

impl TempEvent {
    /// Largest first; ties by time, process, who and fingerprint so the order never depends on
    /// arrival.
    fn order(&self, other: &Self) -> Ordering {
        other
            .bytes
            .cmp(&self.bytes)
            .then_with(|| self.ts.cmp(&other.ts))
            .then_with(|| self.pid.cmp(&other.pid))
            .then_with(|| self.who.cmp(&other.who))
            .then_with(|| self.fingerprint.cmp(&other.fingerprint))
    }
}

/// The `limit` largest temp files seen, largest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TempEvents {
    limit: usize,
    rows: Vec<TempEvent>,
}

impl Default for TempEvents {
    fn default() -> Self {
        Self::new(DEFAULT_KEPT)
    }
}

impl TempEvents {
    /// A list that keeps at most `limit` temp files.
    #[must_use]
    pub fn new(limit: usize) -> Self {
        TempEvents {
            limit,
            rows: Vec::new(),
        }
    }

    /// Offers a temp file; it stays if it is among the largest.
    pub fn offer(&mut self, event: TempEvent) {
        let at = self
            .rows
            .partition_point(|kept| kept.order(&event) != Ordering::Greater);
        if at >= self.limit {
            return;
        }
        self.rows.insert(at, event);
        self.rows.truncate(self.limit);
    }

    /// The kept temp files, largest first.
    #[must_use]
    pub fn rows(&self) -> &[TempEvent] {
        &self.rows
    }
}

impl Merge for TempEvents {
    fn merge(&mut self, other: Self) {
        self.limit = self.limit.max(other.limit);
        for event in other.rows {
            self.offer(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(second: i64, bytes: u64) -> TempEvent {
        TempEvent {
            ts: Timestamp::from_second(1_760_000_000 + second).unwrap(),
            bytes,
            pid: Some(7),
            who: 1,
            fingerprint: 0,
        }
    }

    #[test]
    fn the_largest_stay_and_ties_keep_the_earlier() {
        let mut list = TempEvents::new(3);
        for (second, bytes) in [(5, 10), (1, 50), (4, 30), (2, 30), (3, 70)] {
            list.offer(event(second, bytes));
        }
        let kept: Vec<(i64, u64)> = list
            .rows()
            .iter()
            .map(|e| (e.ts.as_second() - 1_760_000_000, e.bytes))
            .collect();
        assert_eq!(kept, [(3, 70), (1, 50), (2, 30)]);
    }

    #[test]
    fn merging_halves_equals_one_pass() {
        let all: Vec<TempEvent> = (0..20_u32)
            .map(|n| event(i64::from(n), 100 + u64::from(n * 37) % 11))
            .collect();
        let mut whole = TempEvents::new(5);
        let (mut left, mut right) = (TempEvents::new(5), TempEvents::new(5));
        for (n, e) in all.iter().enumerate() {
            whole.offer(e.clone());
            if n % 3 == 0 {
                left.offer(e.clone());
            } else {
                right.offer(e.clone());
            }
        }
        left.merge(right);
        assert_eq!(left, whole);
    }

    #[test]
    fn a_list_stored_without_events_loads_with_the_default_limit() {
        #[derive(Deserialize)]
        struct Holder {
            #[serde(default)]
            events: TempEvents,
        }
        let stored = serde_json::json!({"files": 0});
        let holder: Holder = serde_json::from_value(stored).unwrap();
        assert_eq!(holder.events.limit, DEFAULT_KEPT);
    }
}
