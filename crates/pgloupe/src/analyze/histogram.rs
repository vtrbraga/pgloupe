//! Fixed-edge histograms read from quantile sketches.

use serde::Serialize;

use super::sketch::Sketch;

/// Upper edges, in milliseconds, of the statement duration histogram (pgBadger's).
pub const STATEMENT_EDGES_MS: [f64; 10] = [
    1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 500.0, 1_000.0, 5_000.0, 10_000.0,
];

/// Upper edges, in milliseconds, of the session duration histogram: 1 s to 24 h.
pub const SESSION_EDGES_MS: [f64; 11] = [
    1_000.0,
    10_000.0,
    30_000.0,
    60_000.0,
    300_000.0,
    600_000.0,
    1_800_000.0,
    3_600_000.0,
    10_800_000.0,
    43_200_000.0,
    86_400_000.0,
];

/// Observations in one range, `(previous edge, le_ms]`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HistogramBucket {
    /// Inclusive upper edge in milliseconds; `None` for the last bucket (everything above).
    pub le_ms: Option<f64>,
    /// Observations in the range.
    pub count: u64,
}

/// Splits `sketch` at `edges` (ascending). Counts near an edge carry the sketch's 1% error; the
/// counts always add up to the sketch's total.
#[must_use]
pub fn buckets(sketch: &Sketch, edges: &[f64]) -> Vec<HistogramBucket> {
    let mut rows = Vec::with_capacity(edges.len() + 1);
    let mut below = 0;
    for edge in edges {
        let up_to = sketch.count_at_most(*edge);
        rows.push(HistogramBucket {
            le_ms: Some(*edge),
            count: up_to - below,
        });
        below = up_to;
    }
    rows.push(HistogramBucket {
        le_ms: None,
        count: sketch.count() - below,
    });
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(values: &[f64]) -> Sketch {
        let mut sketch = Sketch::new();
        for value in values {
            sketch.add(*value);
        }
        sketch
    }

    #[test]
    fn counts_fall_in_the_right_ranges_and_add_up() {
        let sketch = filled(&[0.2, 0.5, 3.0, 7.0, 7.5, 60.0, 250.0, 20_000.0]);
        let rows = buckets(&sketch, &STATEMENT_EDGES_MS);
        let counts: Vec<u64> = rows.iter().map(|r| r.count).collect();
        assert_eq!(counts, [2, 1, 2, 0, 0, 1, 1, 0, 0, 0, 1]);
        assert_eq!(counts.iter().sum::<u64>(), sketch.count());
        assert_eq!(rows.last().unwrap().le_ms, None);
    }

    #[test]
    fn an_empty_sketch_gives_empty_buckets() {
        let rows = buckets(&Sketch::new(), &SESSION_EDGES_MS);
        assert_eq!(rows.len(), SESSION_EDGES_MS.len() + 1);
        assert!(rows.iter().all(|r| r.count == 0));
    }

    #[test]
    fn zero_durations_land_in_the_first_bucket() {
        let rows = buckets(&filled(&[0.0, 0.0, 2.0]), &STATEMENT_EDGES_MS);
        assert_eq!((rows[0].count, rows[1].count), (2, 1));
    }
}
