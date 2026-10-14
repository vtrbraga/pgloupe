//! Log analysis: events, fingerprints, mergeable accumulators and reports.

pub mod event;
pub mod fingerprint;
pub mod sketch;

/// Combining two partial results into one.
///
/// Implementations must be associative and commutative so that parallel chunks, incremental runs
/// and roll-ups can all use the same code path.
pub trait Merge {
    /// Folds `other` into `self`.
    fn merge(&mut self, other: Self);
}
