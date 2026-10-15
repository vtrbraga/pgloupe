//! The slowest single executions, kept as a bounded, mergeable top list.

use std::cmp::Ordering;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::Merge;

/// Executions kept unless the caller asks for another number.
pub const DEFAULT_KEPT: usize = 20;
/// Statement text and parameters longer than this are cut.
pub const MAX_TEXT_BYTES: usize = 4_096;

/// One execution of a statement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Execution {
    /// When the log recorded it.
    pub ts: Timestamp,
    /// How long it took, in microseconds.
    pub duration_us: u64,
    /// The fingerprint of its statement (0 for the overflow bucket).
    pub fingerprint: u64,
    /// The statement as the log spelled it, cut at [`MAX_TEXT_BYTES`].
    pub text: String,
    /// Bind parameters, when they were asked for and the log had them.
    pub params: Option<String>,
    pub database: Option<String>,
    pub user: Option<String>,
    pub application: Option<String>,
    pub host: Option<String>,
}

impl Execution {
    /// Slowest first; ties by time, fingerprint and text so the order never depends on arrival.
    fn order(&self, other: &Self) -> Ordering {
        other
            .duration_us
            .cmp(&self.duration_us)
            .then_with(|| self.ts.cmp(&other.ts))
            .then_with(|| self.fingerprint.cmp(&other.fingerprint))
            .then_with(|| self.text.cmp(&other.text))
            .then_with(|| self.params.cmp(&other.params))
            .then_with(|| self.database.cmp(&other.database))
            .then_with(|| self.user.cmp(&other.user))
            .then_with(|| self.application.cmp(&other.application))
            .then_with(|| self.host.cmp(&other.host))
    }
}

/// The `limit` slowest executions seen, slowest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slowest {
    limit: usize,
    rows: Vec<Execution>,
}

impl Slowest {
    /// A list that keeps at most `limit` executions.
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Slowest {
            limit,
            rows: Vec::new(),
        }
    }

    /// Whether an execution of `duration_us` could still make the list. Equal durations might,
    /// through the tie-break.
    #[must_use]
    pub fn wants(&self, duration_us: u64) -> bool {
        self.limit > 0
            && (self.rows.len() < self.limit
                || self
                    .rows
                    .last()
                    .is_some_and(|last| duration_us >= last.duration_us))
    }

    /// Offers an execution; it stays if it is among the slowest.
    pub fn offer(&mut self, execution: Execution) {
        let at = self
            .rows
            .partition_point(|kept| kept.order(&execution) != Ordering::Greater);
        if at >= self.limit {
            return;
        }
        self.rows.insert(at, execution);
        self.rows.truncate(self.limit);
    }

    /// The kept executions, slowest first.
    #[must_use]
    pub fn rows(&self) -> &[Execution] {
        &self.rows
    }
}

impl Merge for Slowest {
    fn merge(&mut self, other: Self) {
        self.limit = self.limit.max(other.limit);
        for execution in other.rows {
            self.offer(execution);
        }
    }
}

/// Who ran a statement: the attributes of the record that logged it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Who {
    pub database: Option<String>,
    pub user: Option<String>,
    pub application: Option<String>,
    pub host: Option<String>,
}

/// A stable 64-bit key of `who`, so examples carry eight bytes instead of four strings.
#[must_use]
pub fn who_key(
    database: Option<&str>,
    user: Option<&str>,
    application: Option<&str>,
    host: Option<&str>,
) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in [database, user, application, host] {
        for byte in part.unwrap_or_default().bytes().chain([0xff]) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}

/// One slow run of a statement, kept small: the people are looked up by `who`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Example {
    pub ts: Timestamp,
    pub duration_us: u64,
    /// Key into the accumulators' table of [`Who`].
    pub who: u64,
    pub params: Option<String>,
}

impl Example {
    fn order(&self, other: &Self) -> Ordering {
        other
            .duration_us
            .cmp(&self.duration_us)
            .then_with(|| self.ts.cmp(&other.ts))
            .then_with(|| self.who.cmp(&other.who))
            .then_with(|| self.params.cmp(&other.params))
    }
}

/// The slowest runs of one statement, slowest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Examples {
    limit: usize,
    rows: Vec<Example>,
}

impl Examples {
    /// A list that keeps at most `limit` runs.
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Examples {
            limit,
            rows: Vec::new(),
        }
    }

    /// Whether a run of `duration_us` could still make the list.
    #[must_use]
    pub fn wants(&self, duration_us: u64) -> bool {
        self.limit > 0
            && (self.rows.len() < self.limit
                || self
                    .rows
                    .last()
                    .is_some_and(|last| duration_us >= last.duration_us))
    }

    /// Offers a run; it stays if it is among the slowest.
    pub fn offer(&mut self, example: Example) {
        let at = self
            .rows
            .partition_point(|kept| kept.order(&example) != Ordering::Greater);
        if at >= self.limit {
            return;
        }
        self.rows.insert(at, example);
        self.rows.truncate(self.limit);
    }

    /// The kept runs, slowest first.
    #[must_use]
    pub fn rows(&self) -> &[Example] {
        &self.rows
    }
}

impl Merge for Examples {
    fn merge(&mut self, other: Self) {
        self.limit = self.limit.max(other.limit);
        for example in other.rows {
            self.offer(example);
        }
    }
}

/// `text` cut at [`MAX_TEXT_BYTES`] on a character boundary.
#[must_use]
pub fn cut(text: &str) -> String {
    if text.len() <= MAX_TEXT_BYTES {
        return text.to_owned();
    }
    let mut end = MAX_TEXT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(us: u64, second: i64, text: &str) -> Execution {
        Execution {
            ts: Timestamp::from_second(1_760_000_000 + second).unwrap(),
            duration_us: us,
            fingerprint: 7,
            text: text.to_owned(),
            params: None,
            database: None,
            user: None,
            application: None,
            host: None,
        }
    }

    fn durations(slowest: &Slowest) -> Vec<u64> {
        slowest.rows().iter().map(|e| e.duration_us).collect()
    }

    #[test]
    fn keeps_the_slowest_in_order() {
        let mut slowest = Slowest::new(3);
        for (i, us) in [5, 9, 1, 7, 3, 9].into_iter().enumerate() {
            slowest.offer(run(us, i64::try_from(i).unwrap(), "q"));
        }
        assert_eq!(durations(&slowest), [9, 9, 7]);
    }

    #[test]
    fn wants_only_what_could_enter() {
        let mut slowest = Slowest::new(2);
        assert!(slowest.wants(1));
        slowest.offer(run(10, 0, "a"));
        slowest.offer(run(20, 1, "b"));
        assert!(!slowest.wants(9));
        assert!(slowest.wants(10), "a tie may win on time");
        assert!(slowest.wants(30));
        assert!(!Slowest::new(0).wants(1_000));
    }

    #[test]
    fn ties_break_by_time_and_text_not_arrival() {
        let items = [
            run(5, 2, "b"),
            run(5, 1, "z"),
            run(5, 1, "a"),
            run(5, 3, "c"),
        ];
        let mut forward = Slowest::new(2);
        let mut backward = Slowest::new(2);
        for item in items.iter().cloned() {
            forward.offer(item);
        }
        for item in items.iter().rev().cloned() {
            backward.offer(item);
        }
        assert_eq!(forward, backward);
        assert_eq!(forward.rows()[0].text, "a");
    }

    #[test]
    fn merging_equals_offering_everything_to_one_list() {
        let all: Vec<Execution> = (0..50_u32)
            .map(|i| run(u64::from(i * 37 % 23), i64::from(i), "q"))
            .collect();
        let mut whole = Slowest::new(5);
        let (mut left, mut right) = (Slowest::new(5), Slowest::new(5));
        for (i, item) in all.iter().enumerate() {
            whole.offer(item.clone());
            if i % 2 == 0 {
                left.offer(item.clone());
            } else {
                right.offer(item.clone());
            }
        }
        let mut other_way = right.clone();
        other_way.merge(left.clone());
        left.merge(right);
        assert_eq!(left, whole);
        assert_eq!(other_way, whole);
    }

    #[test]
    fn text_is_cut_on_a_character_boundary() {
        let text = "é".repeat(MAX_TEXT_BYTES);
        let cut = cut(&text);
        assert!(cut.len() <= MAX_TEXT_BYTES && cut.chars().all(|c| c == 'é'));
        assert_eq!(super::cut("short"), "short");
    }
}
