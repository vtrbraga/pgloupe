//! Statement fingerprints. This module is the only user of `pg_query` (FFI to `libpg_query`,
//! Postgres's own parser), so replacing it later touches nothing else.

/// Display text longer than this is cut.
const MAX_DISPLAY_CHARS: usize = 4_096;

/// The grouping key of a statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fingerprint {
    pub id: u64,
    /// False when `pg_query` could not parse the text; the id is then a hash of its words.
    pub parsed: bool,
}

/// Fingerprints `sql`: equal for statements that differ only in literals, case, whitespace or the
/// length of `IN` lists.
#[must_use]
pub fn fingerprint(sql: &str) -> Fingerprint {
    match pg_query::fingerprint(sql) {
        Ok(found) => Fingerprint {
            id: found.value,
            parsed: true,
        },
        Err(_err) => Fingerprint {
            id: fnv1a(collapse(sql).as_bytes()),
            parsed: false,
        },
    }
}

/// The text shown for a fingerprint: Postgres's normalized form (`$1` placeholders) when the
/// statement parsed, else its whitespace-collapsed text.
#[must_use]
pub fn display_text(sql: &str, parsed: bool) -> String {
    if parsed && let Ok(normalized) = pg_query::normalize(sql) {
        return cut(normalized);
    }
    cut(collapse(sql))
}

fn collapse(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn cut(text: String) -> String {
    if text.chars().count() <= MAX_DISPLAY_CHARS {
        text
    } else {
        text.chars().take(MAX_DISPLAY_CHARS).collect()
    }
}

/// A stable 64-bit hash of a statement's raw text, used to break ties deterministically.
#[must_use]
pub fn sql_hash(sql: &str) -> u64 {
    fnv1a(sql.as_bytes())
}

/// 64-bit FNV-1a.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_case_and_whitespace_do_not_change_the_fingerprint() {
        let a = fingerprint("SELECT * FROM film WHERE film_id = 1");
        let b = fingerprint("select *   from film\nwhere film_id = 99");
        assert_eq!(a, b);
        assert!(a.parsed);
    }

    #[test]
    fn in_lists_of_any_length_collapse() {
        assert_eq!(
            fingerprint("SELECT a FROM t WHERE x IN (1,2,3)"),
            fingerprint("SELECT a FROM t WHERE x IN (1,2,3,4,5)")
        );
    }

    #[test]
    fn different_statements_differ() {
        assert_ne!(
            fingerprint("SELECT * FROM film").id,
            fingerprint("SELECT * FROM actor").id
        );
    }

    #[test]
    fn placeholders_only_statements_parse() {
        let f = fingerprint("UPDATE film SET rental_rate = $1 WHERE film_id = $2");
        assert!(f.parsed);
    }

    #[test]
    fn unparsable_statements_get_a_stable_id() {
        let a = fingerprint("SELEC 1");
        let b = fingerprint("SELEC   1");
        assert!(!a.parsed);
        assert_eq!(a, b, "whitespace-collapsed text is hashed");
        assert_ne!(a.id, fingerprint("SELEC 2").id);
    }

    #[test]
    fn nul_bytes_and_empty_text_do_not_panic() {
        let _ = fingerprint("SELECT 1\0");
        let _ = fingerprint("");
        let _ = display_text("SELECT 1\0", false);
    }

    #[test]
    fn display_text_is_the_normalized_statement() {
        assert_eq!(
            display_text("select * from film where film_id = 42", true),
            "select * from film where film_id = $1"
        );
        assert_eq!(display_text("SELEC   1", false), "SELEC 1");
    }

    #[test]
    fn one_megabyte_statement_is_fingerprinted() {
        let sql = format!("SELECT {}", vec!["1"; 200_000].join(", "));
        assert!(sql.len() > 400_000);
        let f = fingerprint(&sql);
        assert!(f.parsed);
        assert!(display_text(&sql, true).chars().count() <= 4_096);
    }
}
