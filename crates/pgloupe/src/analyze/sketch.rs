//! A mergeable quantile sketch (`DDSketch`) with 1% relative accuracy.

use std::{collections::BTreeMap, sync::LazyLock};

use serde::{Deserialize, Serialize};

use super::Merge;

/// Relative accuracy of every reported quantile (for positive values).
const ALPHA: f64 = 0.01;
/// Buckets kept; beyond this the lowest buckets are merged upwards.
const MAX_BUCKETS: usize = 2_048;

static LN_GAMMA: LazyLock<f64> = LazyLock::new(|| ((1.0 + ALPHA) / (1.0 - ALPHA)).ln());

/// `DDSketch`: values are counted in logarithmic buckets of ratio γ = (1+α)/(1−α), so any quantile
/// is within α of the true value. Merging adds bucket counts, which is associative and
/// commutative.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "SketchRepr", into = "SketchRepr")]
pub struct Sketch {
    buckets: BTreeMap<i32, u64>,
    /// Values ≤ 0 (a duration cannot be negative; they are counted, not bucketed).
    zeros: u64,
    count: u64,
    min: f64,
    max: f64,
}

/// What a [`Sketch`] looks like on disk.
#[derive(Serialize, Deserialize)]
struct SketchRepr {
    buckets: Vec<(i32, u64)>,
    zeros: u64,
    count: u64,
    /// `f64::to_bits`: infinities (an empty sketch) have no JSON form, and bits round trip exactly.
    min_bits: u64,
    max_bits: u64,
}

impl From<Sketch> for SketchRepr {
    fn from(sketch: Sketch) -> Self {
        SketchRepr {
            buckets: sketch.buckets.into_iter().collect(),
            zeros: sketch.zeros,
            count: sketch.count,
            min_bits: sketch.min.to_bits(),
            max_bits: sketch.max.to_bits(),
        }
    }
}

impl From<SketchRepr> for Sketch {
    fn from(repr: SketchRepr) -> Self {
        Sketch {
            buckets: repr.buckets.into_iter().collect(),
            zeros: repr.zeros,
            count: repr.count,
            min: f64::from_bits(repr.min_bits),
            max: f64::from_bits(repr.max_bits),
        }
    }
}

impl Default for Sketch {
    fn default() -> Self {
        Self::new()
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "the index of any finite positive f64 fits in i32 at 1% accuracy"
)]
fn index_of(value: f64) -> i32 {
    (value.ln() / *LN_GAMMA).ceil() as i32
}

/// The value a bucket stands for: the point with the smallest worst-case relative error.
fn value_of(index: i32) -> f64 {
    let gamma = LN_GAMMA.exp();
    2.0 * (f64::from(index) * *LN_GAMMA).exp() / (gamma + 1.0)
}

impl Sketch {
    /// An empty sketch.
    #[must_use]
    pub fn new() -> Self {
        Sketch {
            buckets: BTreeMap::new(),
            zeros: 0,
            count: 0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
        }
    }

    /// Adds one observation. Non-finite values are ignored.
    pub fn add(&mut self, value: f64) {
        if !value.is_finite() {
            return;
        }
        self.count += 1;
        self.min = self.min.min(value);
        self.max = self.max.max(value);
        if value <= 0.0 {
            self.zeros += 1;
        } else {
            *self.buckets.entry(index_of(value)).or_insert(0) += 1;
            self.collapse();
        }
    }

    /// Observations added so far.
    #[must_use]
    pub fn count(&self) -> u64 {
        self.count
    }

    /// Smallest observation, if any.
    #[must_use]
    pub fn min(&self) -> Option<f64> {
        (self.count > 0).then_some(self.min)
    }

    /// Largest observation, if any.
    #[must_use]
    pub fn max(&self) -> Option<f64> {
        (self.count > 0).then_some(self.max)
    }

    /// Buckets currently in use.
    #[must_use]
    pub fn bucket_count(&self) -> usize {
        self.buckets.len()
    }

    /// The value at quantile `q` (0 = minimum, 1 = maximum), within 1% for positive data.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "rank arithmetic on counts far below 2^53"
    )]
    pub fn quantile(&self, q: f64) -> Option<f64> {
        if self.count == 0 {
            return None;
        }
        let rank = (q.clamp(0.0, 1.0) * (self.count - 1) as f64).floor() as u64;
        if rank < self.zeros {
            return Some(0.0_f64.clamp(self.min, self.max));
        }
        let mut seen = self.zeros;
        for (&index, &n) in &self.buckets {
            seen += n;
            if rank < seen {
                return Some(value_of(index).clamp(self.min, self.max));
            }
        }
        Some(self.max)
    }

    /// Merges the lowest buckets upwards until at most [`MAX_BUCKETS`] remain.
    fn collapse(&mut self) {
        while self.buckets.len() > MAX_BUCKETS {
            let Some((_, n)) = self.buckets.pop_first() else {
                return;
            };
            if let Some(next) = self.buckets.first_entry() {
                *next.into_mut() += n;
            }
        }
    }
}

impl Merge for Sketch {
    fn merge(&mut self, other: Self) {
        for (index, n) in other.buckets {
            *self.buckets.entry(index).or_insert(0) += n;
        }
        self.zeros += other.zeros;
        self.count += other.count;
        self.min = self.min.min(other.min);
        self.max = self.max.max(other.max);
        self.collapse();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::approx_eq;

    #[test]
    fn an_empty_and_a_filled_sketch_survive_json() {
        let empty = Sketch::new();
        let json = serde_json::to_string(&empty).unwrap();
        assert_eq!(serde_json::from_str::<Sketch>(&json).unwrap(), empty);
        let mut filled = Sketch::new();
        for v in [0.0, 0.3, 12.5, 12.5, 4000.125] {
            filled.add(v);
        }
        let json = serde_json::to_string(&filled).unwrap();
        assert_eq!(serde_json::from_str::<Sketch>(&json).unwrap(), filled);
    }

    /// A small deterministic generator, so tests need no `rand`.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            f64::from(u32::try_from(self.0 >> 33).unwrap()) / 2_147_483_648.0
        }
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "test arithmetic on small ranks"
    )]
    fn exact(sorted: &[f64], q: f64) -> f64 {
        let rank = (q * (sorted.len() - 1) as f64).floor() as usize;
        sorted[rank]
    }

    fn assert_close(sketch: &Sketch, sorted: &[f64]) {
        for q in [0.0, 0.25, 0.5, 0.9, 0.95, 0.99, 1.0] {
            let got = sketch.quantile(q).unwrap();
            let want = exact(sorted, q);
            assert!(
                (got - want).abs() <= want.abs() * 0.011 + 1e-12,
                "q={q}: got {got}, want {want}"
            );
        }
    }

    fn sketch_of(values: &[f64]) -> Sketch {
        let mut sketch = Sketch::new();
        for v in values {
            sketch.add(*v);
        }
        sketch
    }

    #[test]
    fn quantiles_are_within_one_percent_on_several_distributions() {
        let mut rng = Lcg(7);
        let uniform: Vec<f64> = (0..20_000).map(|_| 0.001 + rng.next() * 500.0).collect();
        let lognormal: Vec<f64> = (0..20_000)
            .map(|_| (rng.next() * 12.0 - 4.0).exp())
            .collect();
        let bimodal: Vec<f64> = (0..20_000)
            .map(|i| {
                if i % 10 == 0 {
                    900.0 + rng.next() * 100.0
                } else {
                    0.5 + rng.next()
                }
            })
            .collect();
        for values in [uniform, lognormal, bimodal] {
            let mut sorted = values.clone();
            sorted.sort_by(f64::total_cmp);
            assert_close(&sketch_of(&values), &sorted);
        }
    }

    #[test]
    fn constant_values_have_exact_quantiles() {
        let sketch = sketch_of(&[12.5; 1_000]);
        for q in [0.0, 0.5, 0.99, 1.0] {
            assert!(approx_eq(sketch.quantile(q).unwrap(), 12.5), "q={q}");
        }
        assert_eq!(sketch.count(), 1_000);
    }

    #[test]
    fn zeros_and_negatives_do_not_break_quantiles() {
        let sketch = sketch_of(&[-5.0, 0.0, 0.0, 10.0, 20.0]);
        assert_eq!(sketch.count(), 5);
        assert!(approx_eq(sketch.min().unwrap(), -5.0));
        assert!(approx_eq(sketch.max().unwrap(), 20.0));
        assert!(sketch.quantile(0.0).unwrap() <= 0.0);
        let top = sketch.quantile(1.0).unwrap();
        assert!((top - 20.0).abs() <= 0.21, "{top}");
    }

    #[test]
    fn empty_and_non_finite_input() {
        let mut sketch = Sketch::new();
        assert_eq!(sketch.quantile(0.5), None);
        assert_eq!(sketch.min(), None);
        sketch.add(f64::NAN);
        sketch.add(f64::INFINITY);
        assert_eq!(sketch.count(), 0);
    }

    #[test]
    fn merging_equals_adding_everything_in_any_split() {
        let mut rng = Lcg(99);
        let values: Vec<f64> = (0..5_000).map(|_| rng.next() * 100.0).collect();
        let whole = sketch_of(&values);
        for split in [0, 1, 17, 2_500, 4_999, 5_000] {
            let (a, b) = values.split_at(split);
            let mut left = sketch_of(a);
            left.merge(sketch_of(b));
            assert_eq!(left, whole, "split at {split}");
            let mut right = sketch_of(b);
            right.merge(sketch_of(a));
            assert_eq!(right, whole, "reversed split at {split}");
        }
    }

    #[test]
    fn merge_is_associative() {
        let mut rng = Lcg(5);
        let chunks: Vec<Vec<f64>> = (0..3)
            .map(|_| (0..300).map(|_| rng.next() * 50.0).collect())
            .collect();
        let [a, b, c] = [&chunks[0], &chunks[1], &chunks[2]].map(|c| sketch_of(c));
        let mut ab_c = a.clone();
        ab_c.merge(b.clone());
        ab_c.merge(c.clone());
        let mut bc = b;
        bc.merge(c);
        let mut a_bc = a;
        a_bc.merge(bc);
        assert_eq!(ab_c, a_bc);
    }

    #[test]
    fn the_bucket_count_is_bounded() {
        let mut sketch = Sketch::new();
        let mut value = 1e-9;
        while value < 1e30 {
            sketch.add(value);
            value *= 1.05;
        }
        assert!(sketch.bucket_count() <= 2_048);
        assert!(sketch.quantile(1.0).unwrap() >= 1e29);
    }
}
