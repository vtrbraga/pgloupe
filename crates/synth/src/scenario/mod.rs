//! Scenario framework: named workloads and the log events they must produce.

pub mod baseline;
pub mod ddl;
pub mod temp_files;

use anyhow::bail;
use postgres::Client;
use rand::SeedableRng as _;
use rand_chacha::ChaCha8Rng;

use crate::{
    manifest::{EventKind, Expect},
    session::Target,
};

/// When a scenario runs relative to snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Between snapshots 0 and 1.
    Main,
    /// Between snapshots 1 and 2; disturbs `pg_stat_statements`.
    Churn,
}

/// Expected count of one event kind, declared by a scenario.
#[derive(Debug, Clone, Copy)]
pub struct ExpectSpec {
    /// Event kind.
    pub event: EventKind,
    /// SQLSTATE for [`EventKind::Error`].
    pub sqlstate: Option<&'static str>,
    /// Minimum count.
    pub min: u32,
    /// Maximum count; `None` means unbounded.
    pub max: Option<u32>,
}

impl ExpectSpec {
    /// Exactly `n` events.
    #[must_use]
    pub const fn exactly(event: EventKind, n: u32) -> Self {
        Self {
            event,
            sqlstate: None,
            min: n,
            max: Some(n),
        }
    }

    /// At least `n` events.
    #[must_use]
    pub const fn at_least(event: EventKind, n: u32) -> Self {
        Self {
            event,
            sqlstate: None,
            min: n,
            max: None,
        }
    }

    /// Between `min` and `max` events, inclusive.
    #[must_use]
    pub const fn between(event: EventKind, min: u32, max: u32) -> Self {
        Self {
            event,
            sqlstate: None,
            min,
            max: Some(max),
        }
    }

    /// Exactly `n` errors with `sqlstate`.
    #[must_use]
    pub const fn error(sqlstate: &'static str, n: u32) -> Self {
        Self {
            event: EventKind::Error,
            sqlstate: Some(sqlstate),
            min: n,
            max: Some(n),
        }
    }
}

/// A named workload.
#[derive(Debug)]
pub struct Scenario {
    /// Name used in `--only` and in `application_name`.
    pub name: &'static str,
    /// When it runs.
    pub phase: Phase,
    /// The workload.
    pub run: fn(&mut Ctx) -> anyhow::Result<()>,
    /// Log events it must produce.
    pub expect: &'static [ExpectSpec],
}

impl Scenario {
    /// `application_name` of this scenario's sessions.
    #[must_use]
    pub fn app(&self) -> String {
        format!("synth:{}", self.name)
    }

    /// Expectations as manifest records.
    #[must_use]
    pub fn expectations(&self) -> Vec<Expect> {
        self.expect
            .iter()
            .map(|spec| Expect {
                scenario: self.name.to_owned(),
                app: self.app(),
                event: spec.event,
                sqlstate: spec.sqlstate.map(str::to_owned),
                min: spec.min,
                max: spec.max,
            })
            .collect()
    }
}

/// State shared with every scenario.
#[derive(Debug)]
pub struct Ctx {
    /// Server address.
    pub target: Target,
    /// RNG seeded per scenario from the run seed and scenario name.
    pub rng: ChaCha8Rng,
    /// `application_name` of the running scenario.
    pub app: String,
    /// Statement texts recorded by the `multiline` scenario.
    pub multiline_texts: Vec<String>,
    seed: u64,
}

impl Ctx {
    /// Context for a run with `seed`.
    #[must_use]
    pub fn new(target: Target, seed: u64) -> Self {
        Self {
            target,
            rng: ChaCha8Rng::seed_from_u64(seed),
            app: String::new(),
            multiline_texts: Vec::new(),
            seed,
        }
    }

    /// Prepares the context for `scenario`: sets the app name and reseeds the RNG.
    pub fn begin(&mut self, scenario: &Scenario) {
        self.app = scenario.app();
        self.rng = ChaCha8Rng::seed_from_u64(scenario_seed(self.seed, scenario.name));
    }

    /// Superuser session on `db` tagged with the scenario's app name.
    ///
    /// # Errors
    ///
    /// Fails if the connection fails.
    pub fn session(&self, db: &str) -> anyhow::Result<Client> {
        self.target.connect(db, "postgres", &self.app)
    }

    /// Unlogged superuser session on `db` for setup and polling.
    ///
    /// # Errors
    ///
    /// Fails if the connection fails.
    pub fn control(&self, db: &str) -> anyhow::Result<Client> {
        self.target.control(db)
    }
}

/// Per-scenario seed so `--only` subsets reproduce the same workload as a full run.
fn scenario_seed(seed: u64, name: &str) -> u64 {
    // FNV-1a over the name, mixed with the run seed.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in name.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash ^ seed
}

/// Every scenario, in run order.
pub const ALL: &[Scenario] = &[baseline::SCENARIO, ddl::SCENARIO, temp_files::SCENARIO];

/// Scenarios to run: all when `only` is empty, else the named ones in canonical order.
///
/// # Errors
///
/// Fails if a name is unknown.
pub fn select(only: &[String]) -> anyhow::Result<Vec<&'static Scenario>> {
    for name in only {
        if !ALL.iter().any(|s| s.name == name) {
            let known: Vec<_> = ALL.iter().map(|s| s.name).collect();
            bail!("unknown scenario {name:?}; known: {}", known.join(", "));
        }
    }
    Ok(ALL
        .iter()
        .filter(|s| only.is_empty() || only.iter().any(|n| n == s.name))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_rejects_unknown_name() {
        let err = select(&["nope".to_owned()]).unwrap_err().to_string();
        assert!(err.contains("nope"), "{err}");
    }

    #[test]
    fn select_all_when_empty() {
        let picked = select(&[]).unwrap();
        assert_eq!(picked.len(), ALL.len());
    }

    #[test]
    fn scenario_names_are_unique() {
        let mut names: Vec<_> = ALL.iter().map(|s| s.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ALL.len());
    }

    #[test]
    fn rng_depends_on_seed_and_scenario_name() {
        assert_eq!(scenario_seed(42, "a"), scenario_seed(42, "a"));
        assert_ne!(scenario_seed(42, "a"), scenario_seed(43, "a"));
        assert_ne!(scenario_seed(42, "a"), scenario_seed(42, "b"));
    }
}
