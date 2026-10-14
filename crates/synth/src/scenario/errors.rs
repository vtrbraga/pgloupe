//! One statement per common SQLSTATE, each checked for the exact error.

use anyhow::{Context as _, bail};

use super::{Ctx, ExpectSpec, Phase, Scenario};

/// (statement, SQLSTATE it must fail with).
const CASES: &[(&str, &str)] = &[
    ("SELEC 1", "42601"),
    (
        "INSERT INTO actor (actor_id, first_name, last_name) VALUES (1, 'DUP', 'DUP')",
        "23505",
    ),
    (
        "INSERT INTO film_actor (actor_id, film_id) VALUES (999999, 1)",
        "23503",
    ),
    ("SELECT 1 / 0", "22012"),
    ("SELECT * FROM synth_no_such_table", "42P01"),
    ("SELECT pg_sleep(5)", "57014"),
];

/// The `errors` scenario.
pub const SCENARIO: Scenario = Scenario {
    name: "errors",
    phase: Phase::Main,
    run,
    expect: &[
        ExpectSpec::error("42601", 1),
        ExpectSpec::error("23505", 1),
        ExpectSpec::error("23503", 1),
        ExpectSpec::error("22012", 1),
        ExpectSpec::error("42P01", 1),
        ExpectSpec::error("57014", 1),
    ],
};

fn run(ctx: &mut Ctx) -> anyhow::Result<()> {
    let mut db = ctx.session("pagila")?;
    db.batch_execute("SET statement_timeout = '100ms'")?;
    for (sql, code) in CASES {
        let err = db
            .batch_execute(sql)
            .err()
            .with_context(|| format!("{sql} should fail with {code}"))?;
        let got = err.code().map(postgres::error::SqlState::code);
        if got != Some(*code) {
            bail!("{sql}: expected SQLSTATE {code}, got {got:?} ({err})");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expectations_cover_every_case_once() {
        let expected: Vec<_> = SCENARIO
            .expect
            .iter()
            .map(|e| e.sqlstate.unwrap())
            .collect();
        let cases: Vec<_> = CASES.iter().map(|(_, code)| *code).collect();
        assert_eq!(expected, cases);
    }
}
