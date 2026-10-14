//! Statements with newlines, comments and multi-line literals.

use anyhow::Context as _;

use super::{Ctx, ExpectSpec, Phase, Scenario};
use crate::manifest::EventKind;

/// Exact texts sent; recorded in the manifest.
pub const TEXTS: &[&str] = &[
    "SELECT title\nFROM film\nWHERE film_id = 1",
    "-- leading comment\nSELECT count(*) FROM actor",
    "SELECT /* block\n   comment */ count(*) FROM customer",
    "SELECT 'line one\nline two' AS two_lines",
    "SELECT\n    f.title,\n    c.name\nFROM film f\n    JOIN film_category fc USING (film_id)\n    JOIN category c USING (category_id)\nWHERE f.film_id = 2",
    "SELECT 1 -- trailing comment\n",
    "WITH recent AS (\n    SELECT * FROM rental ORDER BY rental_date DESC LIMIT 5\n)\nSELECT count(*) FROM recent",
    "SELECT $tag$dollar\nquoted$tag$ AS dq",
    "SELECT E'escaped\\nnewline' AS esc,\n       'second line' AS other",
    "UPDATE film\nSET rental_rate = rental_rate\nWHERE film_id = 3\n",
];

/// The `multiline` scenario.
pub const SCENARIO: Scenario = Scenario {
    name: "multiline",
    phase: Phase::Main,
    run,
    expect: &[ExpectSpec::exactly(EventKind::Statement, 10)],
};

fn run(ctx: &mut Ctx) -> anyhow::Result<()> {
    let mut db = ctx.session("pagila")?;
    for sql in TEXTS {
        db.batch_execute(sql)
            .with_context(|| format!("running {sql:?}"))?;
    }
    ctx.multiline_texts = TEXTS.iter().map(|t| (*t).to_owned()).collect();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_text_spans_lines() {
        assert!(TEXTS.iter().all(|t| t.contains('\n')));
    }

    #[test]
    fn statement_expectation_matches_texts() {
        assert_eq!(SCENARIO.expect[0].min as usize, TEXTS.len());
    }
}
