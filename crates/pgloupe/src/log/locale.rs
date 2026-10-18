//! Reading logs written in another language.
//!
//! PostgreSQL translates its log messages (`lc_messages`). The table in `locales.tsv` pairs each
//! message pgloupe understands with its translation, taken from the server's own catalogs. A
//! translated message is matched against the translated template and rewritten as the English
//! one, so the rest of the analysis never sees another language.
//!
//! EXPERIMENTAL: the table covers Portuguese (Brazil), Spanish, French and German; any other
//! language can be added with a table of the same shape (`--messages-file`).

use std::sync::{LazyLock, RwLock};

use regex::Regex;

use super::Severity;
use crate::hash::FastMap;

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

static SEVERITY_WORDS: LazyLock<FastMap<&'static str, Severity>> = LazyLock::new(|| {
    let mut words = FastMap::default();
    for entry in BUILT_IN.iter() {
        if let Some((_, severity)) = SEVERITIES.iter().find(|(name, _)| *name == entry.english) {
            words.entry(entry.translated.as_str()).or_insert(*severity);
        }
    }
    words
});

/// Labels from the tables given with `--messages-file`. Decoders look labels up without a
/// translator at hand, so they are kept here for the rest of the run.
static EXTRA_LABELS: RwLock<Vec<Entry>> = RwLock::new(Vec::new());

fn remember_labels(entries: &[Entry]) {
    let mut kept = EXTRA_LABELS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for entry in entries {
        if !kept.contains(entry) {
            kept.push(entry.clone());
        }
    }
}

/// The severity a translated label stands for (`ERRO`, `FEHLER`, `ATTENTION`, ...).
#[must_use]
pub fn localized_severity(label: &str) -> Option<Severity> {
    if let Some(severity) = SEVERITY_WORDS.get(label) {
        return Some(*severity);
    }
    let kept = EXTRA_LABELS
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    kept.iter()
        .filter(|e| e.translated == label)
        .find_map(|e| SEVERITIES.iter().find(|(name, _)| *name == e.english))
        .map(|(_, severity)| *severity)
}

const SECONDARY: [&str; 6] = [
    "DETAIL",
    "HINT",
    "QUERY",
    "CONTEXT",
    "STATEMENT",
    "LOCATION",
];

static SECONDARY_WORDS: LazyLock<FastMap<&'static str, &'static str>> = LazyLock::new(|| {
    let mut words = FastMap::default();
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
    if let Some(name) = SECONDARY_WORDS.get(label) {
        return Some(*name);
    }
    let kept = EXTRA_LABELS
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    kept.iter()
        .filter(|e| e.translated.trim_end().trim_end_matches(':') == label)
        .find_map(|e| {
            let english = e.english.trim_end().trim_end_matches(':');
            SECONDARY.iter().find(|name| **name == english).copied()
        })
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

/// Which `%s` of a template stands for a translated word (`execute`), if the template has one.
fn word_slot(english: &str) -> Option<usize> {
    match english {
        "%s %s%s%s: %s" => Some(0),
        "duration: %s ms  %s %s%s%s: %s" => Some(1),
        _ => None,
    }
}

/// `template` with its `n`-th `%s` (0-based) replaced by `word`.
fn fill_nth(template: &str, n: usize, word: &str) -> String {
    let mut out = String::with_capacity(template.len() + word.len());
    let mut seen = 0;
    let mut rest = template;
    while let Some(at) = rest.find("%s") {
        out.push_str(&rest[..at]);
        if seen == n {
            out.push_str(word);
        } else {
            out.push_str("%s");
        }
        seen += 1;
        rest = &rest[at + 2..];
    }
    out.push_str(rest);
    out
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
            let rows = parse_table(text)?;
            remember_labels(&rows);
            entries.extend(rows);
        }
        // `execute` is translated on its own and then put into a message that is not: one rule
        // per word, so the rule can say which word it is and a statement that happens to contain
        // `": "` is not mistaken for one.
        let words: Vec<(String, String)> = entries
            .iter()
            .filter(|e| matches!(e.english.as_str(), "execute" | "execute fetch from"))
            .map(|e| (e.translated.clone(), e.english.clone()))
            .collect();
        let templates: Vec<Entry> = entries
            .iter()
            .filter(|e| word_slot(&e.english).is_some())
            .cloned()
            .collect();
        entries.retain(|e| word_slot(&e.english).is_none());
        for template in &templates {
            let slot = word_slot(&template.english).unwrap_or_default();
            for (translated, english) in &words {
                entries.push(Entry {
                    locale: template.locale.clone(),
                    english: fill_nth(&template.english, slot, english),
                    translated: fill_nth(&template.translated, slot, translated),
                });
            }
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

/// Whether `english` is a severity or secondary label rather than a message.
fn is_label(english: &str) -> bool {
    let name = english.trim_end().trim_end_matches(':');
    SEVERITIES.iter().any(|(n, _)| *n == name) || SECONDARY.contains(&name)
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
            .filter(|e| !is_label(&e.english))
            .map(|e| e.translated.split('%').next().unwrap_or_default())
            .filter(|p| p.chars().count() >= 5 && !p.contains('\n'))
            .collect();
        // A message opens after its `LOG:  ` label (or a quote, in csvlog and jsonlog): the same
        // words inside a statement do not count.
        let hits = sample
            .lines()
            .filter(|line| {
                prefixes.iter().any(|p| {
                    line.match_indices(p).any(|(at, _)| {
                        let before = &line[..at];
                        before.ends_with(":  ") || before.ends_with('"')
                    })
                })
            })
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
    use std::collections::HashMap;

    use super::*;

    fn translator(locale: &str) -> Translator {
        Translator::new(locale, None).unwrap()
    }

    #[test]
    fn a_statement_with_a_colon_in_it_is_still_a_statement() {
        let pt = translator("pt_BR");
        assert_eq!(
            pt.apply(r#"duração: 13.5 ms  comando: SELECT '{"a": 1}'::jsonb"#)
                .as_deref(),
            Some(r#"duration: 13.5 ms  statement: SELECT '{"a": 1}'::jsonb"#)
        );
    }

    #[test]
    fn the_translated_execute_word_is_put_back() {
        for (locale, timed, untimed) in [
            (
                "pt_BR",
                "duração: 2.0 ms  executar S_1/C_2: SELECT 1",
                "executar S_1/C_2: SELECT 1",
            ),
            (
                "de",
                "Dauer: 2.0 ms  Ausführen S_1/C_2: SELECT 1",
                "Ausführen S_1/C_2: SELECT 1",
            ),
        ] {
            let t = translator(locale);
            assert_eq!(
                t.apply(timed).as_deref(),
                Some("duration: 2.0 ms  execute S_1/C_2: SELECT 1"),
                "{locale}"
            );
            assert_eq!(
                t.apply(untimed).as_deref(),
                Some("execute S_1/C_2: SELECT 1"),
                "{locale}"
            );
        }
        let pt = translator("pt_BR");
        assert_eq!(
            pt.apply("duração: 2.0 ms  executar busca de S_1/C_2: SELECT 1")
                .as_deref(),
            Some("duration: 2.0 ms  execute fetch from S_1/C_2: SELECT 1")
        );
        assert_eq!(pt.apply("alguma coisa: qualquer"), None);
    }

    #[test]
    fn english_text_that_mentions_translated_words_is_not_detected() {
        let log = "2026-10-14 10:00:00 UTC [1] LOG:  statement: SELECT * FROM AVISO\n".repeat(5);
        assert_eq!(detect(&log), None);
        let pt = "2026-10-14 10:00:00 UTC [1] LOG:  duração: 1.0 ms  comando: SELECT 1\n".repeat(5);
        assert_eq!(detect(&pt), Some("pt_BR"));
    }

    #[test]
    fn labels_from_an_extra_table_are_known_to_the_decoders() {
        let table = "it\tERROR\tERRORE\nit\tSTATEMENT:  \tISTRUZIONE:  \n";
        Translator::new("it", Some(table)).unwrap();
        assert_eq!(localized_severity("ERRORE"), Some(Severity::Error));
        assert_eq!(localized_secondary("ISTRUZIONE"), Some("STATEMENT"));
    }

    #[test]
    fn no_label_means_two_things_in_the_built_in_languages() {
        let mut seen: HashMap<&str, &str> = HashMap::new();
        for entry in BUILT_IN.iter().filter(|e| is_label(&e.english)) {
            let english = entry.english.trim_end().trim_end_matches(':');
            if let Some(previous) =
                seen.insert(entry.translated.trim_end().trim_end_matches(':'), english)
            {
                assert_eq!(previous, english, "{:?}", entry.translated);
            }
        }
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
