//! Log analysis: events, fingerprints, mergeable accumulators and reports.

pub mod acc;
pub mod diagnostics;
pub mod dimensions;
pub mod event;
pub mod filter;
pub mod fingerprint;
pub mod histogram;
pub mod kind;
pub mod pipeline;
pub mod render;
pub mod report;
pub mod sketch;
pub mod slice;
pub mod slowest;

/// Combining two partial results into one.
///
/// Implementations must be associative and commutative so that parallel chunks, incremental runs
/// and roll-ups can all use the same code path.
pub trait Merge {
    /// Folds `other` into `self`.
    fn merge(&mut self, other: Self);
}
