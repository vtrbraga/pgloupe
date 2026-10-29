//! Statistics broken down by database, user, application and host.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{Merge, acc::to_micros};

/// Values kept per dimension; the rest fold into [`OTHER`].
pub const LIMIT: usize = 1_000;
/// Where values beyond [`LIMIT`] go.
pub const OTHER: &str = "(other)";
/// The label of records that do not carry the attribute.
pub const NONE: &str = "(none)";
/// Values with the most temp bytes that [`Dimensions::recap`] keeps even when they are not busy.
pub const TEMP_KEPT: usize = 20;

/// What one value of a dimension did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DimStats {
    /// Statements run.
    pub statements: u64,
    /// Statements that carried a duration.
    pub timed: u64,
    /// Sum of those durations, in microseconds.
    pub total_us: u64,
    /// Records at `ERROR` or above.
    pub errors: u64,
    /// Connections authorized.
    pub connections: u64,
    /// Sessions that ended.
    pub sessions: u64,
    /// Sum of their durations, in microseconds.
    pub session_us: u64,
    /// Temporary files written.
    #[serde(default)]
    pub temp_files: u64,
    /// Their total size in bytes.
    #[serde(default)]
    pub temp_bytes: u64,
}

impl DimStats {
    fn count(&mut self, tally: Tally) {
        match tally {
            Tally::Statement(duration_ms) => {
                self.statements += 1;
                if let Some(ms) = duration_ms {
                    self.timed += 1;
                    self.total_us += to_micros(ms);
                }
            }
            Tally::Error => self.errors += 1,
            Tally::Connection => self.connections += 1,
            Tally::Session(ms) => {
                self.sessions += 1;
                self.session_us += to_micros(ms);
            }
            Tally::Temp(bytes) => {
                self.temp_files += 1;
                self.temp_bytes += bytes;
            }
        }
    }
}

impl Merge for DimStats {
    fn merge(&mut self, other: Self) {
        self.statements += other.statements;
        self.timed += other.timed;
        self.total_us += other.total_us;
        self.errors += other.errors;
        self.connections += other.connections;
        self.sessions += other.sessions;
        self.session_us += other.session_us;
        self.temp_files += other.temp_files;
        self.temp_bytes += other.temp_bytes;
    }
}

/// One thing a record can be counted under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dimension {
    Database,
    User,
    Application,
    Host,
}

impl Dimension {
    /// Every dimension, in report order.
    pub const ALL: [Dimension; 4] = [
        Dimension::Database,
        Dimension::User,
        Dimension::Application,
        Dimension::Host,
    ];
}

/// What happened in a record, to be counted under each of its attributes.
#[derive(Debug, Clone, Copy)]
pub enum Tally {
    /// A statement, with its duration in milliseconds when it had one.
    Statement(Option<f64>),
    /// A record at `ERROR` or above.
    Error,
    /// A connection was authorized.
    Connection,
    /// A session ended after this many milliseconds.
    Session(f64),
    /// A temporary file of this many bytes was written.
    Temp(u64),
}

/// The record attributes a tally is filed under.
#[derive(Debug, Clone, Copy)]
pub struct Attributes<'a> {
    pub database: Option<&'a str>,
    pub user: Option<&'a str>,
    pub application: Option<&'a str>,
    pub host: Option<&'a str>,
}

/// Per-value statistics for each dimension.
///
/// Merging adds every value; only [`Dimensions::recap`], called once at the end of a run, applies
/// the cap, so the result does not depend on how records were split.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Dimensions {
    pub databases: BTreeMap<String, DimStats>,
    pub users: BTreeMap<String, DimStats>,
    pub applications: BTreeMap<String, DimStats>,
    pub hosts: BTreeMap<String, DimStats>,
}

fn label(value: Option<&str>) -> &str {
    match value {
        Some(text) if !text.is_empty() && text != "[unknown]" => text,
        _ => NONE,
    }
}

impl Dimensions {
    /// The values of one dimension.
    #[must_use]
    pub fn of(&self, dimension: Dimension) -> &BTreeMap<String, DimStats> {
        match dimension {
            Dimension::Database => &self.databases,
            Dimension::User => &self.users,
            Dimension::Application => &self.applications,
            Dimension::Host => &self.hosts,
        }
    }

    fn of_mut(&mut self, dimension: Dimension) -> &mut BTreeMap<String, DimStats> {
        match dimension {
            Dimension::Database => &mut self.databases,
            Dimension::User => &mut self.users,
            Dimension::Application => &mut self.applications,
            Dimension::Host => &mut self.hosts,
        }
    }

    /// Counts `tally` under each attribute of the record it came from.
    pub fn add(&mut self, attributes: &Attributes<'_>, tally: Tally) {
        for (dimension, value) in [
            (Dimension::Database, attributes.database),
            (Dimension::User, attributes.user),
            (Dimension::Application, attributes.application),
            (Dimension::Host, attributes.host),
        ] {
            let map = self.of_mut(dimension);
            let key = label(value);
            // Look up by `&str`: nearly every record repeats a known value, so the common case
            // allocates nothing and searches once.
            if let Some(slot) = map.get_mut(key) {
                slot.count(tally);
                continue;
            }
            let mut slot = DimStats::default();
            slot.count(tally);
            map.insert(key.to_owned(), slot);
        }
    }

    /// Keeps the `limit` busiest values of each dimension, plus the [`TEMP_KEPT`] with most temp
    /// bytes, and folds the rest into [`OTHER`] (ties: the larger name goes first), so the result
    /// does not depend on arrival order.
    pub fn recap(&mut self, limit: usize) {
        for dimension in Dimension::ALL {
            let map = self.of_mut(dimension);
            let kept = limit.max(1);
            if map.len() <= kept {
                continue;
            }
            let mut ranked: Vec<(String, (u64, u64))> = map
                .iter()
                .filter(|(name, _)| name.as_str() != OTHER)
                .map(|(name, stats)| {
                    (
                        name.clone(),
                        (stats.total_us, stats.statements + stats.connections),
                    )
                })
                .collect();
            ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let mut spillers: Vec<(&String, u64)> = map
                .iter()
                .filter(|(name, stats)| name.as_str() != OTHER && stats.temp_bytes > 0)
                .map(|(name, stats)| (name, stats.temp_bytes))
                .collect();
            spillers.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
            let spilled: Vec<String> = spillers
                .into_iter()
                .take(TEMP_KEPT)
                .map(|(name, _)| name.clone())
                .collect();
            for (name, _) in ranked.split_off(kept - 1) {
                if spilled.contains(&name) {
                    continue;
                }
                if let Some(stats) = map.remove(&name) {
                    map.entry(OTHER.to_owned()).or_default().merge(stats);
                }
            }
        }
    }
}

impl Merge for Dimensions {
    fn merge(&mut self, other: Self) {
        for dimension in Dimension::ALL {
            let from = match dimension {
                Dimension::Database => &other.databases,
                Dimension::User => &other.users,
                Dimension::Application => &other.applications,
                Dimension::Host => &other.hosts,
            };
            for (name, stats) in from {
                self.of_mut(dimension)
                    .entry(name.clone())
                    .or_default()
                    .merge(stats.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attrs<'a>(database: &'a str, user: Option<&'a str>) -> Attributes<'a> {
        Attributes {
            database: Some(database),
            user,
            application: None,
            host: Some("10.0.0.1"),
        }
    }

    #[test]
    fn a_tally_counts_under_every_attribute() {
        let mut dims = Dimensions::default();
        dims.add(&attrs("shop", Some("app")), Tally::Statement(Some(2.5)));
        dims.add(&attrs("shop", Some("app")), Tally::Statement(None));
        dims.add(&attrs("shop", None), Tally::Error);
        dims.add(&attrs("shop", Some("app")), Tally::Session(1_500.0));
        let shop = &dims.databases["shop"];
        assert_eq!((shop.statements, shop.timed, shop.total_us), (2, 1, 2_500));
        assert_eq!(
            (shop.errors, shop.sessions, shop.session_us),
            (1, 1, 1_500_000)
        );
        assert_eq!(dims.users["app"].statements, 2);
        assert_eq!(dims.users[NONE].errors, 1);
        assert_eq!(dims.applications[NONE].statements, 2);
        assert_eq!(dims.hosts["10.0.0.1"].statements, 2);
    }

    #[test]
    fn blank_and_unknown_values_are_none() {
        let mut dims = Dimensions::default();
        let blank = Attributes {
            database: Some(""),
            user: Some("[unknown]"),
            application: None,
            host: None,
        };
        dims.add(&blank, Tally::Connection);
        assert_eq!(dims.databases[NONE].connections, 1);
        assert_eq!(dims.users[NONE].connections, 1);
    }

    fn many(count: usize, repeats: impl Fn(usize) -> u64) -> Dimensions {
        let mut dims = Dimensions::default();
        for i in 0..count {
            let name = format!("user{i:04}");
            for _ in 0..repeats(i) {
                dims.add(&attrs("shop", Some(&name)), Tally::Statement(Some(1.0)));
            }
        }
        dims
    }

    #[test]
    fn stats_stored_before_the_temp_columns_still_load() {
        let old = r#"{"statements":1,"timed":0,"total_us":0,"errors":0,"connections":0,"sessions":0,"session_us":0}"#;
        let stats: DimStats = serde_json::from_str(old).unwrap();
        assert_eq!((stats.temp_files, stats.temp_bytes), (0, 0));
    }

    #[test]
    fn a_temp_tally_counts_under_every_attribute() {
        let mut dims = Dimensions::default();
        dims.add(&attrs("shop", Some("app")), Tally::Temp(1_000));
        dims.add(&attrs("shop", Some("app")), Tally::Temp(500));
        let shop = &dims.databases["shop"];
        assert_eq!((shop.temp_files, shop.temp_bytes), (2, 1_500));
        assert_eq!(dims.users["app"].temp_bytes, 1_500);
        assert_eq!(dims.applications[NONE].temp_files, 2);
        assert_eq!(dims.hosts["10.0.0.1"].temp_bytes, 1_500);
    }

    #[test]
    fn recap_keeps_a_value_that_only_spilled() {
        let mut dims = many(1_001, |_| 3);
        dims.add(&attrs("shop", Some("rollup")), Tally::Temp(10_000_000));
        dims.recap(LIMIT);
        assert_eq!(dims.users["rollup"].temp_bytes, 10_000_000);
        assert!(dims.users.len() <= LIMIT + TEMP_KEPT);
    }

    #[test]
    fn recap_keeps_the_busiest_and_folds_the_rest() {
        let mut dims = many(10, |i| 10 - i as u64);
        dims.recap(4);
        let names: Vec<_> = dims.users.keys().cloned().collect();
        assert_eq!(names, ["(other)", "user0000", "user0001", "user0002"]);
        // user0003..user0009 had 7+6+5+4+3+2+1 statements.
        assert_eq!(dims.users[OTHER].statements, 28);
        let all: u64 = dims.users.values().map(|s| s.statements).sum();
        assert_eq!(all, 55);
    }

    #[test]
    fn merging_does_not_depend_on_the_split() {
        let whole = {
            let mut d = many(1_200, |i| 1 + (i % 7) as u64);
            d.recap(LIMIT);
            d
        };
        let mut left = many(1_200, |i| if i % 2 == 0 { 1 + (i % 7) as u64 } else { 0 });
        let right = many(1_200, |i| if i % 2 == 1 { 1 + (i % 7) as u64 } else { 0 });
        left.merge(right);
        left.recap(LIMIT);
        let mut other_way = many(1_200, |i| if i % 2 == 1 { 1 + (i % 7) as u64 } else { 0 });
        other_way.merge(many(
            1_200,
            |i| if i % 2 == 0 { 1 + (i % 7) as u64 } else { 0 },
        ));
        other_way.recap(LIMIT);
        assert_eq!(left.users, whole.users);
        assert_eq!(other_way.users, whole.users);
        assert_eq!(whole.users.len(), LIMIT);
    }

    #[test]
    fn many_shards_merge_to_the_same_breakdown_in_any_order() {
        // 2,500 users, each busier in some shards: folding early would lose them.
        let shard = |k: usize| many(2_500, move |i| ((i * 7 + k * 13) % 11) as u64);
        let mut whole = Dimensions::default();
        for k in 0..6 {
            for (name, stats) in shard(k).users {
                whole.users.entry(name).or_default().merge(stats);
            }
        }
        whole.recap(LIMIT);
        let fold = |order: &[usize]| {
            let mut acc = Dimensions::default();
            for k in order {
                acc.merge(shard(*k));
            }
            acc.recap(LIMIT);
            acc
        };
        assert_eq!(fold(&[0, 1, 2, 3, 4, 5]).users, whole.users);
        assert_eq!(fold(&[5, 3, 1, 0, 2, 4]).users, whole.users);
        // A tree: ((0+1)+(2+3)) + (4+5), each pair recapped as a worker would not.
        let mut left = shard(0);
        left.merge(shard(1));
        let mut mid = shard(2);
        mid.merge(shard(3));
        left.merge(mid);
        let mut right = shard(4);
        right.merge(shard(5));
        left.merge(right);
        left.recap(LIMIT);
        assert_eq!(left.users, whole.users);
    }
}
