//! Seeded mix of Pagila reads, joins, aggregates and writes over the simple protocol.

use anyhow::Context as _;
use rand::RngExt as _;
use rand_chacha::ChaCha8Rng;

use super::{Ctx, ExpectSpec, Phase, Scenario};
use crate::manifest::EventKind;

/// Statements per run.
pub const STATEMENTS: u32 = 500;

const TEMPLATES: usize = 20;
const RATINGS: [&str; 5] = ["G", "PG", "PG-13", "R", "NC-17"];

/// The baseline scenario.
pub const SCENARIO: Scenario = Scenario {
    name: "baseline",
    phase: Phase::Main,
    run,
    expect: &[ExpectSpec::exactly(EventKind::Statement, STATEMENTS)],
};

/// The seeded statement list.
#[must_use]
pub fn statements(rng: &mut ChaCha8Rng) -> Vec<String> {
    (0..STATEMENTS)
        .map(|_| {
            let template = rng.random_range(0..TEMPLATES);
            render(template, rng)
        })
        .collect()
}

fn render(template: usize, rng: &mut ChaCha8Rng) -> String {
    let film: u32 = rng.random_range(1..=1000);
    let actor: u32 = rng.random_range(1..=200);
    let customer: u32 = rng.random_range(1..=599);
    match template {
        0 => format!("SELECT title, rental_rate FROM film WHERE film_id = {film}"),
        1 => format!("SELECT first_name, last_name FROM actor WHERE actor_id = {actor}"),
        2 => format!("SELECT f.title FROM film f JOIN film_actor fa USING (film_id) WHERE fa.actor_id = {actor}"),
        3 => "SELECT c.name, count(*) FROM category c JOIN film_category fc USING (category_id) GROUP BY c.name".to_owned(),
        4 => format!("SELECT customer_id, sum(amount) FROM payment WHERE customer_id = {customer} GROUP BY customer_id"),
        5 => format!(
            "SELECT r.rental_date, f.title FROM rental r JOIN inventory i USING (inventory_id) \
             JOIN film f USING (film_id) WHERE r.customer_id = {customer} ORDER BY r.rental_date DESC LIMIT 10"
        ),
        6 => "SELECT count(*) FROM rental WHERE return_date IS NULL".to_owned(),
        7 => {
            let length: u32 = rng.random_range(60..=180);
            let rating = RATINGS[rng.random_range(0..RATINGS.len())];
            format!("SELECT film_id, title FROM film WHERE length > {length} AND rating = '{rating}' LIMIT 20")
        }
        8 => "SELECT store_id, count(*) FROM inventory GROUP BY store_id".to_owned(),
        9 => {
            let city: u32 = rng.random_range(1..=600);
            format!("SELECT ci.city, co.country FROM city ci JOIN country co USING (country_id) WHERE ci.city_id = {city}")
        }
        10 => {
            let letter = char::from(b'A' + rng.random_range(0..26u8));
            format!("SELECT film_id, title FROM film WHERE title ILIKE '{letter}%' ORDER BY title LIMIT 10")
        }
        11 => {
            let month: u32 = rng.random_range(1..=6);
            format!(
                "SELECT avg(amount) FROM payment WHERE payment_date >= '2022-0{month}-01' \
                 AND payment_date < '2022-0{month}-01'::date + interval '1 month'"
            )
        }
        12 => {
            let store: u32 = rng.random_range(1..=2);
            format!("SELECT * FROM film_in_stock({film}, {store})")
        }
        13 => {
            let inventory: u32 = rng.random_range(1..=4581);
            format!("SELECT inventory_in_stock({inventory})")
        }
        14 => "SELECT a.last_name, count(*) FROM actor a JOIN film_actor fa USING (actor_id) \
               GROUP BY a.last_name ORDER BY 2 DESC LIMIT 5"
            .to_owned(),
        15 => format!("SELECT * FROM customer_list WHERE id = {customer}"),
        16 => format!("UPDATE film SET rental_rate = rental_rate WHERE film_id = {film}"),
        17 => format!("UPDATE customer SET active = active WHERE customer_id = {customer}"),
        18 => {
            let staff: u32 = rng.random_range(1..=2);
            let rental: u32 = rng.random_range(1..=16_044);
            let cents: u32 = rng.random_range(99..=1199);
            format!(
                "INSERT INTO payment (customer_id, staff_id, rental_id, amount, payment_date) \
                 VALUES ({customer}, {staff}, {rental}, {}.{:02}, '2022-07-15 12:00:00+00')",
                cents / 100,
                cents % 100
            )
        }
        _ => format!("DELETE FROM payment WHERE payment_id = -{film}"),
    }
}

fn run(ctx: &mut Ctx) -> anyhow::Result<()> {
    let sqls = statements(&mut ctx.rng);
    let mut db = ctx.session("pagila")?;
    for sql in &sqls {
        db.batch_execute(sql)
            .with_context(|| format!("running {sql}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use rand::SeedableRng as _;

    use super::*;

    fn list(seed: u64) -> Vec<String> {
        statements(&mut ChaCha8Rng::seed_from_u64(seed))
    }

    #[test]
    fn same_seed_same_statements() {
        assert_eq!(list(42), list(42));
    }

    #[test]
    fn different_seed_different_statements() {
        assert_ne!(list(42), list(43));
    }

    #[test]
    fn produces_declared_count_covering_every_template() {
        let sqls = list(42);
        assert_eq!(sqls.len(), STATEMENTS as usize);
        let templates: HashSet<_> = sqls
            .iter()
            .map(|s| s.split_whitespace().take(4).collect::<Vec<_>>())
            .collect();
        assert!(
            templates.len() >= 15,
            "only {} distinct prefixes",
            templates.len()
        );
    }

    #[test]
    fn every_template_renders_valid_looking_sql() {
        let mut rng = ChaCha8Rng::seed_from_u64(1);
        for template in 0..TEMPLATES {
            let sql = render(template, &mut rng);
            assert!(!sql.contains('{'), "unrendered placeholder in {sql}");
            assert!(
                ["SELECT", "UPDATE", "INSERT", "DELETE"]
                    .iter()
                    .any(|verb| sql.starts_with(verb)),
                "{sql}"
            );
        }
    }
}
