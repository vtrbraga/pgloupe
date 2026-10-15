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

static QUOTED: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"(?s)[eEnNbBxX]?'(?:[^']|'')*'|\$([A-Za-z_]*)\$.*?\$([A-Za-z_]*)\$"));
static KEY_VALUE: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"\((?P<cols>[^)]*)\)=\((?P<vals>[^)]*)\)"));
static EMAIL: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}"));
static IPV4: LazyLock<Regex> = LazyLock::new(|| pattern(r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b"));
static SESSION_FIELDS: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"\b(?P<key>user|database|host|application_name)=(?P<value>[^\s,]+)"));

/// Replaces what identifies a person or a customer's data in `text`: quoted and dollar-quoted
/// literals, the values in `Key (id)=(42) already exists`, email addresses and IPv4 addresses.
#[must_use]
pub fn scrub(text: &str) -> String {
    let text = QUOTED.replace_all(text, |caps: &regex::Captures<'_>| {
        if caps.get(1).is_some() {
            format!("${}${}$", &caps[1], &caps[2])
        } else {
            "'?'".to_owned()
        }
    });
    let text = KEY_VALUE.replace_all(&text, "($cols)=(?)");
    let text = EMAIL.replace_all(&text, "?@?");
    IPV4.replace_all(&text, "?.?.?.?").into_owned()
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
                format!("{kind}_{:06x}", hash & 0xff_ffff)
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
        for field in [
            &mut record.detail,
            &mut record.hint,
            &mut record.context,
            &mut record.internal_query,
            &mut record.statement,
        ] {
            if let Some(text) = field.take() {
                *field = Some(self.text(&text));
            }
        }
    }

    /// `scrub`, plus the `user=`, `database=`, `host=` and `application_name=` fields that
    /// connection messages spell out.
    fn text(&mut self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        for caps in SESSION_FIELDS.captures_iter(text) {
            let (Some(whole), Some(key), Some(value)) =
                (caps.get(0), caps.name("key"), caps.name("value"))
            else {
                continue;
            };
            out.push_str(&scrub(&text[last..whole.start()]));
            let kind = match key.as_str() {
                "user" => "user",
                "database" => "db",
                "host" => "host",
                _ => "app",
            };
            let pseudonym = self.pseudonym(kind, value.as_str().trim_end_matches([':', ')']));
            out.push_str(key.as_str());
            out.push('=');
            out.push_str(&pseudonym);
            last = whole.end();
        }
        out.push_str(&scrub(&text[last..]));
        out
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
            "INSERT INTO t VALUES ('?', '?', $q$q$, 42)"
        );
        assert_eq!(
            scrub("duplicate key: Key (email, org)=(bob@x.com, 7) already exists."),
            "duplicate key: Key (email, org)=(?) already exists."
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
        assert!(r.message.contains("u_email"), "constraint names stay");
    }
}
