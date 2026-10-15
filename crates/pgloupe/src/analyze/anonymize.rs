//! Making a log safe to share: names become stable pseudonyms and literals disappear.
//!
//! This runs on records as they are read, before anything is counted, so every output (text, JSON,
//! HTML, the store) agrees and none can leak what the others hide.

use std::{collections::HashMap, sync::LazyLock};

use regex::Regex;
use xxhash_rust::xxh3::xxh3_64_with_seed;

use crate::log::LogRecord;

/// A pattern written in this file; a typo is a bug caught by the first test that runs it.
#[expect(clippy::expect_used, reason = "constant pattern")]
fn pattern(text: &str) -> Regex {
    Regex::new(text).expect("constant pattern is valid")
}

static EMAIL: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}"));
static IPV4: LazyLock<Regex> = LazyLock::new(|| pattern(r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b"));
static IPV6: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        r"(?i)\b(?:[0-9a-f]{1,4}:){7}[0-9a-f]{1,4}\b|\b(?:[0-9a-f]{1,4}(?::[0-9a-f]{1,4})*)?::(?:[0-9a-f]{1,4}(?::[0-9a-f]{1,4})*)?(?:\b|$)",
    )
});
static APPLICATION: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"application_name=(?P<value>.*?)(?: SSL enabled| GSS|$)"));
static SESSION_FIELDS: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"\b(?P<key>user|database|host)=(?P<value>[^\s,]+)"));

/// Where a quote that opens at `at` closes, doubled quotes being part of the text.
fn closing(text: &str, at: usize, quote: char, backslash: bool) -> Option<usize> {
    let mut chars = text[at + 1..].char_indices();
    while let Some((offset, c)) = chars.next() {
        let here = at + 1 + offset;
        if backslash && c == '\\' {
            chars.next();
        } else if c == quote {
            if text[here + 1..].starts_with(quote) {
                chars.next();
            } else {
                return Some(here + 1);
            }
        }
    }
    None
}

/// The end of a `$tag$ ... $tag$` string that opens at `at`.
fn dollar_end(text: &str, at: usize) -> Option<usize> {
    let rest = &text[at + 1..];
    let tag_len = rest.find('$')?;
    let tag = &rest[..tag_len];
    let valid = tag
        .chars()
        .enumerate()
        .all(|(i, c)| c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()));
    if !valid {
        return None;
    }
    let delimiter = &text[at..at + tag_len + 2];
    let body = at + delimiter.len();
    text[body..]
        .find(delimiter)
        .map(|n| body + n + delimiter.len())
}

/// Walks `text`, replacing string literals with `'?'` (and, when `names` is set, double-quoted
/// text with `"?"`), dropping comments. A quote with no closing partner is left alone.
fn scrub_quotes(text: &str, names: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut previous = ' ';
    while i < text.len() {
        let Some(c) = text[i..].chars().next() else {
            break;
        };
        let end = match c {
            '\'' => closing(text, i, '\'', matches!(previous, 'e' | 'E')).map(|e| (e, "'?'")),
            '"' if names => closing(text, i, '"', false).map(|e| (e, "\"?\"")),
            '$' => dollar_end(text, i).map(|e| (e, "'?'")),
            '-' if text[i..].starts_with("--") => {
                Some((text[i..].find('\n').map_or(text.len(), |n| i + n), ""))
            }
            '/' if text[i..].starts_with("/*") => {
                Some((text[i..].find("*/").map_or(text.len(), |n| i + n + 2), ""))
            }
            _ => None,
        };
        if let Some((end, replacement)) = end {
            out.push_str(replacement);
            i = end;
            previous = ' ';
        } else {
            out.push(c);
            i += c.len_utf8();
            previous = c;
        }
    }
    out
}

/// Replaces the group that follows each `marker` (which ends in `(`), nesting included, with `(?)`.
fn scrub_groups(text: &str, marker: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(marker) {
        let open = at + marker.len() - 1;
        out.push_str(&rest[..open]);
        let mut depth = 0_u32;
        let mut close = None;
        for (offset, c) in rest[open..].char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(open + offset + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        if let Some(close) = close {
            out.push_str("(?)");
            rest = &rest[close..];
        } else {
            out.push_str(&rest[open..]);
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

fn mask_addresses(text: &str) -> String {
    let text = EMAIL.replace_all(text, "?@?");
    let text = IPV4.replace_all(&text, "?.?.?.?");
    IPV6.replace_all(&text, "?:?").into_owned()
}

/// A statement made safe to show: literals and comments gone, quoted identifiers kept.
#[must_use]
pub fn scrub(text: &str) -> String {
    mask_addresses(&scrub_quotes(text, false))
}

/// A server message made safe to show: quoted text of either kind, the values in `Key (a)=(b)` and
/// `Failing row contains (...)`, emails and addresses are removed.
#[must_use]
pub fn scrub_message(text: &str) -> String {
    let text = scrub_quotes(text, true);
    let text = scrub_groups(&text, ")=(");
    let text = scrub_groups(&text, "contains (");
    mask_addresses(&text)
}

/// Scrubs the statements of a lock wait.
pub fn scrub_pair(pair: &mut crate::log::BlockPair) {
    pair.blocker = pair.blocker.take().map(|s| scrub(&s));
    pair.waiter = pair.waiter.take().map(|s| scrub(&s));
}

/// Rewrites records in place.
#[derive(Debug, Clone)]
pub struct Anonymizer {
    seed: u64,
    cache: HashMap<(&'static str, String), String>,
}

impl Anonymizer {
    /// An anonymizer whose pseudonyms depend on `key`: the same key gives the same pseudonyms on
    /// every run, another key gives different ones.
    #[must_use]
    pub fn new(key: &str) -> Self {
        Anonymizer {
            seed: xxh3_64_with_seed(key.as_bytes(), 0x7067_6c6f),
            cache: HashMap::new(),
        }
    }

    /// `kind_3f9a2c`: the same input always maps to the same pseudonym.
    fn pseudonym(&mut self, kind: &'static str, value: &str) -> String {
        let seed = self.seed;
        self.cache
            .entry((kind, value.to_owned()))
            .or_insert_with(|| {
                let hash = xxh3_64_with_seed(value.as_bytes(), seed);
                format!("{kind}_{:010x}", hash & 0xff_ffff_ffff)
            })
            .clone()
    }

    /// Anonymizes names, text and literals of `record`.
    pub fn apply(&mut self, record: &mut LogRecord) {
        for (kind, field) in [
            ("user", &mut record.user),
            ("db", &mut record.database),
            ("app", &mut record.application),
            ("host", &mut record.host),
        ] {
            if let Some(value) = field.take() {
                let pseudonym = self.pseudonym(kind, &value);
                *field = Some(pseudonym);
            }
        }
        record.message = self.text(&record.message);
        if let Some(c) = &mut record.correlation {
            c.sql = scrub(&c.sql);
        }
        if let Some(statement) = record.statement.take() {
            record.statement = Some(scrub(&statement));
        }
        if let Some(query) = record.internal_query.take() {
            record.internal_query = Some(scrub(&query));
        }
        for pair in &mut record.blocks {
            scrub_pair(pair);
        }
        for field in [&mut record.detail, &mut record.hint, &mut record.context] {
            if let Some(text) = field.take() {
                *field = Some(self.text(&text));
            }
        }
    }

    /// A server message with its values removed, and the `user=`, `database=`, `host=` and
    /// `application_name=` fields that connection messages spell out turned into pseudonyms.
    fn text(&mut self, text: &str) -> String {
        let mut named = String::with_capacity(text.len());
        let mut last = 0;
        let mut fields: Vec<(std::ops::Range<usize>, &'static str, String)> = Vec::new();
        for caps in SESSION_FIELDS.captures_iter(text) {
            if let (Some(whole), Some(key), Some(value)) =
                (caps.get(0), caps.name("key"), caps.name("value"))
            {
                let kind = match key.as_str() {
                    "user" => "user",
                    "database" => "db",
                    _ => "host",
                };
                fields.push((whole.range(), kind, value.as_str().to_owned()));
            }
        }
        if let Some(caps) = APPLICATION.captures(text)
            && let (Some(whole), Some(value)) = (caps.get(0), caps.name("value"))
        {
            fields.push((whole.range(), "app", value.as_str().to_owned()));
        }
        fields.sort_by_key(|(range, _, _)| range.start);
        for (range, kind, value) in fields {
            if range.start < last {
                continue;
            }
            named.push_str(&scrub_message(&text[last..range.start]));
            let key = match kind {
                "user" => "user",
                "db" => "database",
                "host" => "host",
                _ => "application_name",
            };
            let pseudonym = self.pseudonym(kind, value.trim_end_matches([':', ')']));
            named.push_str(key);
            named.push('=');
            named.push_str(&pseudonym);
            last = range.end;
        }
        named.push_str(&scrub_message(&text[last..]));
        named
    }
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;

    use super::*;
    use crate::log::Severity;

    #[test]
    fn literals_values_emails_and_addresses_are_scrubbed() {
        assert_eq!(
            scrub("INSERT INTO t VALUES ('o''brien', E'x\\ny', $q$secret$q$, 42)"),
            "INSERT INTO t VALUES ('?', E'?', '?', 42)"
        );
        assert_eq!(
            scrub("SELECT 1 -- don't tell\nFROM \"Users\" /* ssn 123 */ WHERE a = 'x'"),
            "SELECT 1 \nFROM \"Users\"  WHERE a = '?'"
        );
        assert_eq!(scrub("SELECT $1, $2 -- x"), "SELECT $1, $2 ");
        assert_eq!(
            scrub_message("duplicate key: Key (email, org)=(bob@x.com, 7) already exists."),
            "duplicate key: Key (email, org)=(?) already exists."
        );
        assert_eq!(
            scrub_message("Key (name)=(Acme (Holdings) Ltd) already exists."),
            "Key (name)=(?) already exists."
        );
        assert_eq!(
            scrub_message("Failing row contains (17, Jane Doe, null)."),
            "Failing row contains (?)."
        );
        assert_eq!(
            scrub_message("password authentication failed for user \"alice\""),
            "password authentication failed for user \"?\""
        );
        assert_eq!(
            scrub_message("no pg_hba.conf entry for host \"2001:db8::77\", user \"mallory\""),
            "no pg_hba.conf entry for host \"?\", user \"?\""
        );
        assert_eq!(
            scrub_message("from 2001:db8::77 at 10:20:30"),
            "from ?:? at 10:20:30"
        );
        assert_eq!(
            scrub("mail bob@example.com from 10.1.2.3"),
            "mail ?@? from ?.?.?.?"
        );
        assert_eq!(scrub("plain text, 12 rows"), "plain text, 12 rows");
    }

    #[test]
    fn names_get_stable_pseudonyms_that_depend_on_the_key() {
        let mut first = Anonymizer::new("k");
        let mut same = Anonymizer::new("k");
        let mut other = Anonymizer::new("other");
        let record = || {
            let mut r = LogRecord::new(
                Timestamp::UNIX_EPOCH,
                Severity::Log,
                "connection authorized: user=bob database=shop application_name=psql",
            );
            r.user = Some("bob".to_owned());
            r.database = Some("shop".to_owned());
            r.host = Some("10.0.0.5".to_owned());
            r
        };
        let (mut one, mut two, mut three) = (record(), record(), record());
        first.apply(&mut one);
        same.apply(&mut two);
        other.apply(&mut three);
        assert_eq!(one, two, "same key, same pseudonyms");
        assert_ne!(one.user, three.user, "another key, other pseudonyms");
        let user = one.user.clone().unwrap();
        assert!(user.starts_with("user_") && !user.contains("bob"));
        assert!(
            one.message.contains(&format!("user={user}")),
            "{}",
            one.message
        );
        assert!(!one.message.contains("shop") && !one.message.contains("psql"));
        assert!(!format!("{one:?}").contains("10.0.0.5"));
    }

    #[test]
    fn statement_and_detail_text_lose_their_literals() {
        let mut anonymizer = Anonymizer::new("k");
        let mut r = LogRecord::new(
            Timestamp::UNIX_EPOCH,
            Severity::Error,
            "duplicate key value violates unique constraint \"u_email\"",
        );
        r.detail = Some("Key (email)=(bob@x.com) already exists.".to_owned());
        r.statement = Some("INSERT INTO u(email) VALUES ('bob@x.com')".to_owned());
        anonymizer.apply(&mut r);
        assert_eq!(r.detail.as_deref(), Some("Key (email)=(?) already exists."));
        assert_eq!(
            r.statement.as_deref(),
            Some("INSERT INTO u(email) VALUES ('?')")
        );
        assert!(
            !r.message.contains("u_email"),
            "quoted names go too: {}",
            r.message
        );
    }
}
