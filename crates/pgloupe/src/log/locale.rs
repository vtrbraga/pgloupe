//! Reading logs written in another language.
//!
//! PostgreSQL translates its log messages (`lc_messages`). The table in `locales.tsv` pairs each
//! message pgloupe understands with its translation, taken from the server's own catalogs. A
//! translated message is matched against the translated template and rewritten as the English
//! one, so the rest of the analysis never sees another language.
//!
//! EXPERIMENTAL: the table covers Portuguese (Brazil), Spanish, French and German; any other
//! language can be added with a table of the same shape (`--messages-file`).

use std::{collections::HashMap, sync::LazyLock};

use regex::Regex;

use super::Severity;

/// The translations, one per line: `locale<TAB>english<TAB>translated`.
const TABLE: &str = include_str!("locales.tsv");

/// One row of a table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub locale: String,
    pub english: String,
    pub translated: String,
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Parses a table: `#` lines and blank lines are skipped.
///
/// # Errors
///
/// Fails on a line that does not have three tab-separated columns.
pub fn parse_table(text: &str) -> Result<Vec<Entry>, String> {
    let mut entries = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let mut columns = line.split('\t');
        let (Some(locale), Some(english), Some(translated), None) = (
            columns.next(),
            columns.next(),
            columns.next(),
            columns.next(),
        ) else {
            return Err(format!(
                "line {}: expected locale, english and translated, separated by tabs",
                index + 1
            ));
        };
        entries.push(Entry {
            locale: locale.to_owned(),
            english: unescape(english),
            translated: unescape(translated),
        });
    }
    Ok(entries)
}

static BUILT_IN: LazyLock<Vec<Entry>> = LazyLock::new(|| parse_table(TABLE).unwrap_or_default());

/// The languages the built-in table covers.
#[must_use]
pub fn languages() -> Vec<&'static str> {
    let mut found: Vec<&'static str> = BUILT_IN.iter().map(|e| e.locale.as_str()).collect();
    found.sort_unstable();
    found.dedup();
    found
}

const SEVERITIES: [(&str, Severity); 8] = [
    ("DEBUG", Severity::Debug),
    ("INFO", Severity::Info),
    ("NOTICE", Severity::Notice),
    ("WARNING", Severity::Warning),
    ("ERROR", Severity::Error),
    ("LOG", Severity::Log),
    ("FATAL", Severity::Fatal),
    ("PANIC", Severity::Panic),
];

static SEVERITY_WORDS: LazyLock<HashMap<&'static str, Severity>> = LazyLock::new(|| {
    let mut words = HashMap::new();
    for entry in BUILT_IN.iter() {
        if let Some((_, severity)) = SEVERITIES.iter().find(|(name, _)| *name == entry.english) {
            words.entry(entry.translated.as_str()).or_insert(*severity);
        }
    }
    words
});

/// The severity a translated label stands for (`ERRO`, `FEHLER`, `ATTENTION`, ...).
#[must_use]
pub fn localized_severity(label: &str) -> Option<Severity> {
    SEVERITY_WORDS.get(label).copied()
}

const SECONDARY: [&str; 6] = [
    "DETAIL",
    "HINT",
    "QUERY",
    "CONTEXT",
    "STATEMENT",
    "LOCATION",
];

static SECONDARY_WORDS: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    let mut words = HashMap::new();
    for entry in BUILT_IN.iter() {
        if let Some(name) = SECONDARY
            .iter()
            .find(|name| entry.english.trim_end().trim_end_matches(':') == **name)
        {
            words
                .entry(entry.translated.trim_end().trim_end_matches(':'))
                .or_insert(*name);
        }
    }
    words
});

/// The English name (`HINT`, `CONTEXT`, ...) of a translated secondary label.
#[must_use]
pub fn localized_secondary(label: &str) -> Option<&'static str> {
    SECONDARY_WORDS.get(label).copied()
}

/// A piece of a message template.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece {
    Text(String),
    /// The n-th argument (0-based) and how to read it.
    Arg(usize, Kind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Integer,
    Decimal,
    Hex,
}

/// Splits a `printf` template into text and conversions. `%%` is a literal percent sign.
fn pieces(template: &str) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut sequential = 0;
    let chars: Vec<char> = template.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '%' {
            text.push(chars[i]);
            i += 1;
            continue;
        }
        if chars.get(i + 1) == Some(&'%') {
            text.push('%');
            i += 2;
            continue;
        }
        let mut j = i + 1;
        let digits: String = chars[j..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        let mut position = None;
        if chars.get(j + digits.len()) == Some(&'$') && !digits.is_empty() {
            position = digits.parse::<usize>().ok().map(|n| n.saturating_sub(1));
            j += digits.len() + 1;
        }
        while j < chars.len() && !chars[j].is_ascii_alphabetic() {
            j += 1;
        }
        while j < chars.len() && matches!(chars[j], 'l' | 'h' | 'z' | 'q') {
            j += 1;
        }
        let Some(&conversion) = chars.get(j) else {
            text.push_str(&chars[i..].iter().collect::<String>());
            break;
        };
        let kind = match conversion {
            'd' | 'i' | 'u' => Kind::Integer,
            'f' | 'g' | 'e' => Kind::Decimal,
            'x' | 'X' => Kind::Hex,
            // Strings, characters and anything unfamiliar are read as text.
            _ => Kind::Text,
        };
        if !text.is_empty() {
            out.push(Piece::Text(std::mem::take(&mut text)));
        }
        let index = position.unwrap_or(sequential);
        sequential += 1;
        out.push(Piece::Arg(index, kind));
        i = j + 1;
    }
    if !text.is_empty() {
        out.push(Piece::Text(text));
    }
    out
}

/// One message: how to recognize its translation and how to say it in English.
#[derive(Debug)]
struct Rule {
    /// What the translated message starts with, before any argument: a cheap first check.
    prefix: String,
    pattern: Regex,
    /// Argument number each capture group holds.
    groups: Vec<usize>,
    english: Vec<Piece>,
}

fn build(entry: &Entry) -> Option<Rule> {
    let translated = pieces(&entry.translated);
    let mut pattern = String::from("(?s)^");
    let mut groups = Vec::new();
    let mut prefix = String::new();
    let mut seen_arg = false;
    for (n, piece) in translated.iter().enumerate() {
        match piece {
            Piece::Text(text) => {
                pattern.push_str(&regex::escape(text));
                if !seen_arg {
                    prefix.push_str(text);
                }
            }
            Piece::Arg(index, kind) => {
                seen_arg = true;
                let last = n + 1 == translated.len();
                let group = match kind {
                    Kind::Integer => r"(-?\d+)",
                    Kind::Decimal => r"(\d+(?:[.,]\d+)?)",
                    Kind::Hex => "([0-9A-Fa-f]+)",
                    Kind::Text if last => "(.*)",
                    Kind::Text => "(.*?)",
                };
                pattern.push_str(group);
                groups.push(*index);
            }
        }
    }
    pattern.push('$');
    Some(Rule {
        prefix,
        pattern: Regex::new(&pattern).ok()?,
        groups,
        english: pieces(&entry.english),
    })
}

/// Rewrites one language's messages as English.
#[derive(Debug, Default)]
pub struct Translator {
    rules: Vec<Rule>,
    /// Matches the language's ` at character N` suffix: group 1 is the message, group 2 the number.
    position: Option<Regex>,
}

impl Translator {
    /// The rules for `locale` from the built-in table and, if given, `extra` (a table of the
    /// same shape; its rows apply whatever their locale column says).
    ///
    /// # Errors
    ///
    /// Fails when `extra` is not a table.
    pub fn new(locale: &str, extra: Option<&str>) -> Result<Translator, String> {
        let mut entries: Vec<Entry> = BUILT_IN
            .iter()
            .filter(|e| e.locale == locale)
            .cloned()
            .collect();
        if let Some(text) = extra {
            entries.extend(parse_table(text)?);
        }
        // Longer templates first, so a message is not taken for a shorter one it starts with.
        entries.sort_by_key(|e| std::cmp::Reverse(e.translated.len()));
        let position = entries
            .iter()
            .find(|e| e.english == " at character %d")
            .and_then(|e| {
                let (before, after) = e.translated.split_once("%d")?;
                Regex::new(&format!(
                    r"(?s)^(.*){}(\d+){}$",
                    regex::escape(before),
                    regex::escape(after)
                ))
                .ok()
            });
        Ok(Translator {
            rules: entries.iter().filter_map(build).collect(),
            position,
        })
    }

    /// Whether there is anything to translate with.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// `message` without the language's ` at character N` suffix, and the number.
    #[must_use]
    pub fn split_position<'a>(&self, message: &'a str) -> Option<(&'a str, u32)> {
        let caps = self.position.as_ref()?.captures(message)?;
        let number = caps.get(2)?.as_str().parse().ok()?;
        Some((caps.get(1)?.as_str(), number))
    }

    /// The English form of `message`, or `None` if no rule recognizes it.
    #[must_use]
    pub fn apply(&self, message: &str) -> Option<String> {
        for rule in &self.rules {
            if !message.starts_with(&rule.prefix) {
                continue;
            }
            let Some(caps) = rule.pattern.captures(message) else {
                continue;
            };
            let argument = |n: usize| -> &str {
                rule.groups
                    .iter()
                    .position(|g| *g == n)
                    .and_then(|group| caps.get(group + 1))
                    .map_or("", |m| m.as_str())
            };
            let mut out = String::with_capacity(message.len());
            for piece in &rule.english {
                match piece {
                    Piece::Text(text) => out.push_str(text),
                    // Decimal commas (`200,370`) become the dot the parsers expect.
                    Piece::Arg(n, Kind::Integer | Kind::Decimal) => {
                        out.push_str(&argument(*n).replace(',', "."));
                    }
                    Piece::Arg(n, _) => out.push_str(argument(*n)),
                }
            }
            return Some(out);
        }
        None
    }
}

/// The language `sample` (a few kilobytes of log text) is written in, if it is one of the
/// built-in ones and clearly so.
#[must_use]
pub fn detect(sample: &str) -> Option<&'static str> {
    let mut best: Option<(&'static str, usize)> = None;
    for locale in languages() {
        let prefixes: Vec<&str> = BUILT_IN
            .iter()
            .filter(|e| e.locale == locale)
            .map(|e| e.translated.split('%').next().unwrap_or_default())
            .filter(|p| p.chars().count() >= 5 && !p.contains('\n'))
            .collect();
        let hits = sample
            .lines()
            .filter(|line| prefixes.iter().any(|p| line.contains(p)))
            .count();
        if hits > best.map_or(0, |(_, n)| n) {
            best = Some((locale, hits));
        }
    }
    best.filter(|(_, hits)| *hits >= 3)
        .map(|(locale, _)| locale)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn translator(locale: &str) -> Translator {
        Translator::new(locale, None).unwrap()
    }

    #[test]
    fn a_localized_error_position_is_split_off() {
        let es = translator("es");
        assert_eq!(
            es.split_position("no existe la relación «t» en carácter 15"),
            Some(("no existe la relación «t»", 15))
        );
        let de = translator("de");
        assert_eq!(
            de.split_position("Relation »t« existiert nicht bei Zeichen 7"),
            Some(("Relation »t« existiert nicht", 7))
        );
        assert_eq!(es.split_position("sin posición"), None);
        assert_eq!(es.split_position("en carácter x"), None);
    }

    #[test]
    fn a_translated_message_becomes_its_english_template() {
        let pt = translator("pt_BR");
        assert_eq!(
            pt.apply("duração: 12.345 ms  comando: SELECT 1").as_deref(),
            Some("duration: 12.345 ms  statement: SELECT 1")
        );
        assert_eq!(
            pt.apply("arquivo temporário: caminho \"base/pgsql_tmp/x.0\", tamanho 2800000")
                .as_deref(),
            Some("temporary file: path \"base/pgsql_tmp/x.0\", size 2800000")
        );
        assert_eq!(
            pt.apply(
                "desconexão: tempo de sessão: 0:00:01.234 usuário=u banco de dados=d hospedeiro=h"
            )
            .as_deref(),
            Some("disconnection: session time: 0:00:01.234 user=u database=d host=h")
        );
        assert_eq!(pt.apply("a message nobody translated"), None);
    }

    #[test]
    fn decimal_commas_and_different_punctuation_are_handled() {
        let de = translator("de");
        assert_eq!(
            de.apply("Prozess 12 wartet immer noch auf ShareLock-Sperre auf transaction 5 nach 200,370 ms")
                .as_deref(),
            Some("process 12 still waiting for ShareLock on transaction 5 after 200.370 ms")
        );
        let fr = translator("fr");
        assert_eq!(
            fr.apply("durée : 3.5 ms, instruction : SELECT 2")
                .as_deref(),
            Some("duration: 3.5 ms  statement: SELECT 2")
        );
    }

    #[test]
    fn severity_and_secondary_labels_are_known_in_every_language() {
        assert_eq!(localized_severity("ERRO"), Some(Severity::Error));
        assert_eq!(localized_severity("FEHLER"), Some(Severity::Error));
        assert_eq!(localized_severity("ERREUR"), Some(Severity::Error));
        assert_eq!(localized_severity("ATTENTION"), Some(Severity::Warning));
        assert_eq!(localized_severity("nonsense"), None);
        assert_eq!(localized_secondary("TIPP"), Some("HINT"));
        assert_eq!(localized_secondary("ZUSAMMENHANG"), Some("CONTEXT"));
    }

    #[test]
    fn the_language_of_a_log_is_detected_from_a_few_lines() {
        let sample = "x LOG:  duração: 1 ms  comando: SELECT 1\n\
                      x LOG:  duração: 2 ms  comando: SELECT 2\n\
                      x LOG:  desconexão: tempo de sessão: 0:00:01.000 usuário=u banco de dados=d hospedeiro=h\n";
        assert_eq!(detect(sample), Some("pt_BR"));
        assert_eq!(
            detect("x LOG:  duration: 1 ms  statement: SELECT 1\n"),
            None
        );
        assert_eq!(languages(), ["de", "es", "fr", "pt_BR"]);
    }

    #[test]
    fn an_extra_table_adds_a_language() {
        let extra = "xx\tduration: %s ms\tdurasso: %s ms\n";
        let t = Translator::new("xx", Some(extra)).unwrap();
        assert_eq!(t.apply("durasso: 7 ms").as_deref(), Some("duration: 7 ms"));
        Translator::new("xx", Some("only two\tcolumns\n")).unwrap_err();
    }
}
