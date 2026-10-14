//! Shared helpers for unit tests.

/// True when `a` and `b` differ by less than 1e-9.
pub(crate) fn approx_eq(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}
