//! Statement fingerprints. This module is the only user of `pg_query` (FFI to `libpg_query`,
//! Postgres's own parser), so replacing it later touches nothing else.

/// Display text longer than this is cut.
const MAX_DISPLAY_CHARS: usize = 4_096;

use std::{
    collections::HashMap,
    sync::{LazyLock, PoisonError, RwLock},
};

use xxhash_rust::xxh3::{Xxh3DefaultBuilder, xxh3_64};

use super::kind::QueryKind;

/// The grouping key of a statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fingerprint {
    pub id: u64,
    /// False when `pg_query` could not parse the text; the id is then a hash of its words.
    pub parsed: bool,
}

/// Statement shapes remembered in all. Real workloads repeat a few thousand shapes; a log of
/// many tenants' tables can have a hundred thousand.
const CACHE_LIMIT: usize = 262_144;

/// A shape key longer than this is parsed every time rather than kept.
const MAX_CACHED_KEY: usize = 8 * 1024;

/// The cache is split so that threads rarely wait for each other.
const SHARDS: usize = 64;

type Shard = RwLock<HashMap<String, Fingerprint, Xxh3DefaultBuilder>>;

static CACHE: LazyLock<Vec<Shard>> = LazyLock::new(|| {
    (0..SHARDS)
        .map(|_| RwLock::new(HashMap::with_hasher(Xxh3DefaultBuilder)))
        .collect()
});

fn shard(key: &str) -> &'static Shard {
    let slot = xxh3_64(key.as_bytes()) % (SHARDS as u64);
    &CACHE[usize::try_from(slot).unwrap_or(0)]
}

/// Fingerprints `sql`: equal for statements that differ only in literals, case, whitespace or the
/// length of `IN` lists.
///
/// Parsing is the dominant cost of a run, and nearly every log line repeats a statement shape with
/// different literals, so a cache shared by all threads remembers the fingerprint of each
/// `shape_key`. Statements the key cannot describe safely are parsed every time.
#[must_use]
pub fn fingerprint(sql: &str) -> Fingerprint {
    let Some(key) = shape_key(sql) else {
        return fingerprint_uncached(sql);
    };
    let shard = shard(&key);
    let known = shard
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
        .copied();
    if let Some(found) = known {
        return found;
    }
    let found = fingerprint_uncached(sql);
    // An unparsed statement's id hashes its literals, so it cannot stand for its shape.
    if found.parsed && key.len() <= MAX_CACHED_KEY {
        let mut shard = shard.write().unwrap_or_else(PoisonError::into_inner);
        if shard.len() >= CACHE_LIMIT / SHARDS {
            shard.clear();
        }
        shard.insert(key, found);
    }
    found
}

#[cfg(test)]
fn is_cached(sql: &str) -> bool {
    shape_key(sql).is_some_and(|key| {
        shard(&key)
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(&key)
    })
}

/// The fingerprint straight from `pg_query`, the reference the cache must agree with.
#[must_use]
pub fn fingerprint_uncached(sql: &str) -> Fingerprint {
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

/// `sql` with plain literals replaced by typed placeholders (`?i` integer, `?f` decimal, `?s`
/// string), or `None` when the text holds anything whose meaning a simple scan could get wrong.
///
/// Two statements with the same key differ only in the values of those literals, which
/// `pg_query`'s fingerprint ignores. Only data-manipulation statements are keyed: utility
/// statements can carry numbers that do matter (`WITH (fillfactor = 70)`).
fn shape_key(sql: &str) -> Option<String> {
    let bytes = sql.as_bytes();
    let word: &[u8] = {
        let trimmed = sql.trim_start().as_bytes();
        let end = trimmed
            .iter()
            .position(|b| !b.is_ascii_alphabetic())
            .unwrap_or(trimmed.len());
        &trimmed[..end]
    };
    let dml = ["select", "insert", "update", "delete", "with", "values"];
    if !dml
        .iter()
        .any(|kw| word.eq_ignore_ascii_case(kw.as_bytes()))
    {
        return None;
    }
    let mut key = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let prev = i.checked_sub(1).map(|p| bytes[p]);
        let after_word = prev.is_some_and(|p| p.is_ascii_alphanumeric() || p == b'_');
        match b {
            b'\'' => {
                // `E'..'`, `B'..'`, `X'..'`, `U&'..'` and `type'..'` have their own rules.
                if after_word || prev == Some(b'&') {
                    return None;
                }
                i = skip_quoted(bytes, i, b'\'')?;
                key.extend_from_slice(b"?s");
            }
            b'"' => {
                let end = skip_quoted(bytes, i, b'"')?;
                key.extend_from_slice(&bytes[i..end]);
                i = end;
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => return None,
            b'/' if bytes.get(i + 1) == Some(&b'*') => return None,
            b'$' => {
                // `$1` is a parameter; anything else is dollar quoting.
                if !bytes.get(i + 1).is_some_and(u8::is_ascii_digit) {
                    return None;
                }
                key.push(b'$');
                i += 1;
                while bytes.get(i).is_some_and(u8::is_ascii_digit) {
                    key.push(bytes[i]);
                    i += 1;
                }
            }
            b'0'..=b'9' if !after_word => {
                let (end, class) = scan_number(bytes, i)?;
                key.extend_from_slice(class);
                i = end;
            }
            b'.' if !after_word
                && prev != Some(b')')
                && prev != Some(b'"')
                && bytes.get(i + 1).is_some_and(u8::is_ascii_digit) =>
            {
                let (end, _) = scan_number(bytes, i)?;
                key.extend_from_slice(b"?f");
                i = end;
            }
            _ => {
                // Everything up to the next byte that needs a decision is copied as it is.
                let run = bytes[i..]
                    .iter()
                    .position(|b| NEEDS_DECISION[usize::from(*b)])
                    .map_or(bytes.len() - i, |at| at.max(1));
                key.extend_from_slice(&bytes[i..i + run]);
                i += run;
            }
        }
    }
    String::from_utf8(key).ok()
}

/// The bytes `shape_key` treats specially: quotes, comment starts, `$`, digits and `.`.
const NEEDS_DECISION: [bool; 256] = {
    let mut table = [false; 256];
    let special = b"'\"-/$.0123456789";
    let mut i = 0;
    while i < special.len() {
        table[special[i] as usize] = true;
        i += 1;
    }
    table
};

/// The index just past the quote that closes the one at `start` (a doubled quote is an escape).
fn skip_quoted(bytes: &[u8], start: usize, quote: u8) -> Option<usize> {
    let mut i = start + 1;
    while i < bytes.len() {
        if bytes[i] == quote {
            if bytes.get(i + 1) == Some(&quote) {
                i += 2;
                continue;
            }
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

/// Scans a decimal number starting at `start`; returns where it ends and its placeholder.
/// `None` for forms with other rules (hex, underscores, huge integers, trailing letters).
fn scan_number(bytes: &[u8], start: usize) -> Option<(usize, &'static [u8])> {
    let digits = |from: usize| {
        bytes[from..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count()
    };
    let mut i = start;
    let whole = digits(i);
    i += whole;
    let mut decimal = whole == 0;
    if bytes.get(i) == Some(&b'.') {
        let fraction = digits(i + 1);
        if fraction == 0 {
            return None;
        }
        i += 1 + fraction;
        decimal = true;
    }
    if matches!(bytes.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(bytes.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        let exponent = digits(j);
        if exponent == 0 {
            return None;
        }
        i = j + exponent;
        decimal = true;
    }
    if bytes
        .get(i)
        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'.')
    {
        return None;
    }
    if decimal {
        Some((i, b"?f"))
    } else if whole <= 9 {
        Some((i, b"?i"))
    } else {
        None
    }
}

/// The class of `sql`. The first word settles the common cases; anything else (a `WITH` query,
/// text that starts with a comment, utility statements) is parsed.
#[must_use]
pub fn statement_kind(sql: &str) -> QueryKind {
    let trimmed = sql.trim_start();
    let end = trimmed
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(trimmed.len());
    let word = trimmed[..end].to_ascii_lowercase();
    match word.as_str() {
        "select" | "values" | "table" => QueryKind::Select,
        "insert" => QueryKind::Insert,
        "update" => QueryKind::Update,
        "delete" => QueryKind::Delete,
        "copy" => QueryKind::Copy,
        "begin" | "commit" | "rollback" | "savepoint" | "release" | "abort" | "end" => {
            QueryKind::Tcl
        }
        _ => parsed_kind(sql),
    }
}

/// The command tag of a schema-changing statement (`CREATE TABLE`, `CREATE TEMP TABLE`,
/// `ALTER TABLE`, `DROP`, ...), or `None` for anything else. Meant for statements already known
/// to be DDL, so it parses.
#[must_use]
pub fn statement_tag(sql: &str) -> Option<String> {
    use pg_query::NodeEnum as N;
    let parsed = pg_query::parse(sql).ok()?;
    let node = parsed
        .protobuf
        .stmts
        .first()?
        .stmt
        .as_ref()?
        .node
        .as_ref()?;
    let temporary = |persistence: &str| persistence == "t";
    let tag = match node {
        N::CreateStmt(create) => {
            let temp = create
                .relation
                .as_ref()
                .is_some_and(|r| temporary(&r.relpersistence));
            if temp {
                "CREATE TEMP TABLE"
            } else {
                "CREATE TABLE"
            }
        }
        N::CreateTableAsStmt(create) => {
            let temp = create
                .into
                .as_ref()
                .and_then(|into| into.rel.as_ref())
                .is_some_and(|r| temporary(&r.relpersistence));
            if temp {
                "CREATE TEMP TABLE"
            } else {
                "CREATE TABLE AS"
            }
        }
        N::AlterTableStmt(_) => "ALTER TABLE",
        N::DropStmt(_) => "DROP",
        N::IndexStmt(_) => "CREATE INDEX",
        N::ViewStmt(_) => "CREATE VIEW",
        N::TruncateStmt(_) => "TRUNCATE",
        N::RenameStmt(_) => "RENAME",
        N::CreateSchemaStmt(_) => "CREATE SCHEMA",
        N::CreateSeqStmt(_) | N::AlterSeqStmt(_) => "SEQUENCE",
        N::CreateFunctionStmt(_) | N::AlterFunctionStmt(_) => "FUNCTION",
        N::CreateTrigStmt(_) => "CREATE TRIGGER",
        N::CreateExtensionStmt(_) | N::AlterExtensionStmt(_) => "EXTENSION",
        N::CommentStmt(_) => "COMMENT",
        _ => "OTHER DDL",
    };
    Some(tag.to_owned())
}

fn parsed_kind(sql: &str) -> QueryKind {
    use pg_query::NodeEnum as N;
    let Ok(parsed) = pg_query::parse(sql) else {
        return QueryKind::Other;
    };
    let Some(node) = parsed
        .protobuf
        .stmts
        .first()
        .and_then(|stmt| stmt.stmt.as_ref())
        .and_then(|stmt| stmt.node.as_ref())
    else {
        return QueryKind::Other;
    };
    match node {
        N::SelectStmt(_) => QueryKind::Select,
        N::InsertStmt(_) => QueryKind::Insert,
        N::UpdateStmt(_) => QueryKind::Update,
        N::DeleteStmt(_) => QueryKind::Delete,
        N::CopyStmt(_) => QueryKind::Copy,
        N::TransactionStmt(_) => QueryKind::Tcl,
        N::MergeStmt(_) => QueryKind::Other,
        N::CreateStmt(_)
        | N::CreateTableAsStmt(_)
        | N::AlterTableStmt(_)
        | N::DropStmt(_)
        | N::IndexStmt(_)
        | N::ViewStmt(_)
        | N::TruncateStmt(_)
        | N::RenameStmt(_)
        | N::CreateSchemaStmt(_)
        | N::CreateSeqStmt(_)
        | N::AlterSeqStmt(_)
        | N::CreateFunctionStmt(_)
        | N::AlterFunctionStmt(_)
        | N::CreateTrigStmt(_)
        | N::CreateExtensionStmt(_)
        | N::AlterExtensionStmt(_)
        | N::AlterOwnerStmt(_)
        | N::CommentStmt(_)
        | N::CreateEnumStmt(_)
        | N::CreateDomainStmt(_)
        | N::CreatedbStmt(_)
        | N::DropdbStmt(_)
        | N::CreateRoleStmt(_)
        | N::AlterRoleStmt(_)
        | N::DropRoleStmt(_)
        | N::GrantStmt(_)
        | N::GrantRoleStmt(_)
        | N::CreatePolicyStmt(_)
        | N::AlterPolicyStmt(_)
        | N::ReindexStmt(_)
        | N::ClusterStmt(_) => QueryKind::Ddl,
        _ => QueryKind::Utility,
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
    fn schema_changes_get_a_command_tag_and_temporary_tables_are_told_apart() {
        let tag = |sql: &str| statement_tag(sql).unwrap();
        assert_eq!(tag("CREATE TABLE t (id int)"), "CREATE TABLE");
        assert_eq!(tag("CREATE TEMP TABLE t (id int)"), "CREATE TEMP TABLE");
        assert_eq!(
            tag("CREATE TEMPORARY TABLE t AS SELECT 1"),
            "CREATE TEMP TABLE"
        );
        assert_eq!(tag("CREATE TABLE t AS SELECT 1"), "CREATE TABLE AS");
        assert_eq!(tag("ALTER TABLE t ADD COLUMN c int"), "ALTER TABLE");
        assert_eq!(tag("DROP TABLE t"), "DROP");
        assert_eq!(tag("CREATE INDEX i ON t (id)"), "CREATE INDEX");
        assert_eq!(statement_tag("SELEC"), None);
    }

    #[test]
    fn statements_are_classified() {
        let kind = |sql: &str| statement_kind(sql);
        assert_eq!(kind("SELECT 1"), QueryKind::Select);
        assert_eq!(kind("  select * from t"), QueryKind::Select);
        assert_eq!(kind("INSERT INTO t VALUES (1)"), QueryKind::Insert);
        assert_eq!(kind("update t set a = 1"), QueryKind::Update);
        assert_eq!(kind("DELETE FROM t"), QueryKind::Delete);
        assert_eq!(kind("COPY t FROM STDIN"), QueryKind::Copy);
        assert_eq!(kind("BEGIN"), QueryKind::Tcl);
        assert_eq!(kind("commit"), QueryKind::Tcl);
        assert_eq!(kind("CREATE TABLE t (a int)"), QueryKind::Ddl);
        assert_eq!(kind("ALTER TABLE t ADD COLUMN b int"), QueryKind::Ddl);
        assert_eq!(kind("DROP INDEX i"), QueryKind::Ddl);
        assert_eq!(kind("TRUNCATE t"), QueryKind::Ddl);
        assert_eq!(kind("VACUUM t"), QueryKind::Utility);
        assert_eq!(kind("SET work_mem = '1MB'"), QueryKind::Utility);
        assert_eq!(kind("EXPLAIN SELECT 1"), QueryKind::Utility);
        assert_eq!(kind("SELEC 1"), QueryKind::Other);
        assert_eq!(kind(""), QueryKind::Other);
    }

    #[test]
    fn a_with_query_is_classified_by_its_main_statement() {
        let kind = |sql: &str| statement_kind(sql);
        assert_eq!(
            kind("WITH x AS (SELECT 1) SELECT * FROM x"),
            QueryKind::Select
        );
        assert_eq!(
            kind("WITH x AS (SELECT 1 AS a) INSERT INTO t SELECT a FROM x"),
            QueryKind::Insert
        );
        assert_eq!(
            kind("WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d"),
            QueryKind::Select
        );
        assert_eq!(kind("/* tag */ UPDATE t SET a = 1"), QueryKind::Update);
    }

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

    #[test]
    fn shape_keys_strip_only_plain_literals() {
        let key = |sql: &str| shape_key(sql);
        assert_eq!(
            key("SELECT * FROM t WHERE id = 5").as_deref(),
            Some("SELECT * FROM t WHERE id = ?i")
        );
        assert_eq!(
            key("select a from t where n = 'it''s' and m > 1.5e-3 and k < -7").as_deref(),
            Some("select a from t where n = ?s and m > ?f and k < -?i")
        );
        // Digits inside identifiers, parameters and quoted identifiers are not literals.
        assert_eq!(
            key("SELECT c2 FROM t1 WHERE \"X 1\" = $1 AND d = 3").as_deref(),
            Some("SELECT c2 FROM t1 WHERE \"X 1\" = $1 AND d = ?i")
        );
        assert_eq!(
            key("INSERT INTO t VALUES (1, 'a', 2.5)").as_deref(),
            Some("INSERT INTO t VALUES (?i, ?s, ?f)")
        );
        assert_eq!(
            key("  WITH x AS (SELECT 1) SELECT * FROM x").as_deref(),
            Some("  WITH x AS (SELECT ?i) SELECT * FROM x")
        );
    }

    #[test]
    fn shape_keys_give_up_on_anything_unusual() {
        for sql in [
            "SELECT 1 -- comment",
            "SELECT /* c */ 1",
            "SELECT $$text$$",
            "SELECT $tag$text$tag$",
            "SELECT E'a\\nb'",
            "SELECT B'101'",
            "SELECT X'1F'",
            "SELECT U&'d\\0061t\\+000061'",
            "SELECT date'2020-01-01'",
            "SELECT 0x1F",
            "SELECT 1_000",
            "SELECT 3000000000",
            "SELECT 1abc",
            "SELECT 'unterminated",
            "SELECT \"unterminated",
            "CREATE INDEX i ON t (a) WITH (fillfactor = 70)",
            "SET work_mem = 64",
            "EXPLAIN SELECT 1",
            "",
        ] {
            assert_eq!(shape_key(sql), None, "{sql}");
        }
    }

    #[test]
    fn statements_with_equal_keys_have_equal_fingerprints() {
        let groups: [&[&str]; 5] = [
            &[
                "SELECT * FROM t WHERE id = 1",
                "SELECT * FROM t WHERE id = 999999",
            ],
            &[
                "SELECT * FROM t WHERE n = 'a'",
                "SELECT * FROM t WHERE n = 'it''s ok'",
            ],
            &[
                "SELECT * FROM t WHERE x > 1.5",
                "SELECT * FROM t WHERE x > 2e10",
            ],
            &[
                "UPDATE t SET a = 1, b = 'x' WHERE id = 3",
                "UPDATE t SET a = 2, b = 'y' WHERE id = 4",
            ],
            &[
                "INSERT INTO t VALUES (1, 'a')",
                "INSERT INTO t VALUES (22, 'bb')",
            ],
        ];
        for group in groups {
            let keys: Vec<_> = group.iter().map(|sql| shape_key(sql)).collect();
            assert!(
                keys.iter().all(|k| k.is_some() && *k == keys[0]),
                "{group:?}: {keys:?}"
            );
            let ids: Vec<_> = group.iter().map(|sql| fingerprint_uncached(sql)).collect();
            assert!(ids.iter().all(|f| *f == ids[0]), "{group:?}: {ids:?}");
        }
    }

    #[test]
    fn the_cache_never_changes_a_fingerprint() {
        let statements = [
            "SELECT * FROM film WHERE film_id = 1",
            "SELECT * FROM film WHERE film_id = 2",
            "select * from film where film_id = 3",
            "SELECT a FROM t WHERE x IN (1,2,3)",
            "SELECT a FROM t WHERE x IN (1,2,3,4,5)",
            "SELECT a FROM t WHERE x IN ('a','b')",
            "SELECT * FROM events_p0001 WHERE id = 1",
            "SELECT * FROM events_p0002 WHERE id = 2",
            "SELECT * FROM tenant_01.t WHERE id = 1",
            "SELECT * FROM tenant_02.t WHERE id = 1",
            "UPDATE film SET rental_rate = $1 WHERE film_id = $2",
            "UPDATE film SET rental_rate = 2.99 WHERE film_id = 7",
            "INSERT INTO t (a, b) VALUES (1, 'x'), (2, 'y')",
            "DELETE FROM t WHERE a = 'x' AND b < 10",
            "SELECT 1",
            "SELECT 'a'",
            "SELECT 1.5",
            "SELECT NULL",
            "SELECT * FROM t ORDER BY 1",
            "SELECT * FROM t ORDER BY 2",
            "SELECT * FROM t LIMIT 5",
            "SELECT * FROM t LIMIT 50",
            "SELECT * FROM t WHERE a = 1 -- trailing",
            "SELEC 1",
            "SELEC 2",
            "SELECT '2020-01-01'::date",
            "SELECT interval '1 day'",
            "SELECT * FROM t1, t2 WHERE t1.c1 = t2.c2 AND t1.c1 = 5",
        ];
        for round in 0..3 {
            for sql in statements {
                assert_eq!(
                    fingerprint(sql),
                    fingerprint_uncached(sql),
                    "round {round}: {sql}"
                );
            }
        }
    }

    #[test]
    fn unparsable_statements_are_not_cached_across_literals() {
        assert_ne!(fingerprint("SELEC 1").id, fingerprint("SELEC 2").id);
        assert_ne!(fingerprint("SELEC 1").id, fingerprint("SELEC 2").id);
    }

    #[test]
    fn a_shape_parsed_on_one_thread_is_known_to_the_others() {
        let sql = "SELECT shared_cache_probe FROM t_probe WHERE id = 41";
        assert!(!is_cached(sql));
        let seen = std::thread::spawn(move || fingerprint(sql)).join().unwrap();
        assert!(is_cached(
            "SELECT shared_cache_probe FROM t_probe WHERE id = 42"
        ));
        assert_eq!(
            fingerprint("SELECT shared_cache_probe FROM t_probe WHERE id = 43"),
            seen
        );
    }
}
