//! Extracts the translations of the server messages pgloupe understands from PostgreSQL's own
//! message catalogs (`postgres-<major>.mo`), for every supported major version.
//!
//! The result is a tab-separated table checked in at `crates/pgloupe/src/log/locales.tsv`:
//! `locale<TAB>english<TAB>translated`, with newlines written as `\n`.

use std::{collections::BTreeSet, fmt::Write as _, fs, path::Path};

use anyhow::{Context as _, bail};

use crate::container::docker;

/// Languages extracted, as the directory names under `/usr/share/locale`.
pub const LANGUAGES: &[&str] = &["pt_BR", "es", "fr", "de"];

/// Major versions whose catalogs are read.
pub const MAJORS: &[u32] = &[14, 15, 16, 17, 18];

/// Messages kept: the ones that start with one of these.
const PREFIXES: &[&str] = &[
    "duration: ",
    "connection received",
    "connection authenticated",
    "connection authorized",
    "disconnection: ",
    "temporary file: ",
    "process %d still waiting",
    "process %d acquired",
    "process %d avoided deadlock",
    "deadlock detected",
    "checkpoint complete",
    "restartpoint complete",
    "automatic vacuum of table",
    "automatic aggressive vacuum",
    "automatic analyze of table",
    "automatic aggressive analyze",
    "canceling statement due to",
    "canceling autovacuum task",
    "terminating connection due to",
    "terminating connection because",
    "unexpected EOF on client connection",
    "could not receive data from client",
    "could not send data to client",
    "Process holding the lock",
    "Processes holding the lock",
];

/// Words and labels kept whole.
const WORDS: &[&str] = &[
    "ERROR",
    "WARNING",
    "FATAL",
    "PANIC",
    "NOTICE",
    "INFO",
    "LOG",
    "DEBUG",
    "DETAIL:  ",
    "HINT:  ",
    "QUERY:  ",
    "CONTEXT:  ",
    "STATEMENT:  ",
    "LOCATION:  ",
    "execute",
    "parse",
    "bind",
    "execute fetch from",
];

/// The `msgid -> msgstr` pairs of a `.mo` file.
///
/// # Errors
///
/// Fails if the bytes are not a little- or big-endian `.mo` file.
pub fn read_mo(bytes: &[u8]) -> anyhow::Result<Vec<(String, String)>> {
    let word = |at: usize, big: bool| -> anyhow::Result<usize> {
        let raw: [u8; 4] = bytes
            .get(at..at + 4)
            .and_then(|b| b.try_into().ok())
            .context("truncated .mo file")?;
        let n = if big {
            u32::from_be_bytes(raw)
        } else {
            u32::from_le_bytes(raw)
        };
        Ok(usize::try_from(n)?)
    };
    let big = match word(0, false)? {
        0x9504_12de => false,
        0xde12_0495 => true,
        other => bail!("not a .mo file (magic {other:#x})"),
    };
    let (count, originals, translations) = (word(8, big)?, word(12, big)?, word(16, big)?);
    let string = |table: usize, i: usize| -> anyhow::Result<String> {
        let (length, offset) = (word(table + 8 * i, big)?, word(table + 8 * i + 4, big)?);
        let slice = bytes
            .get(offset..offset + length)
            .context("truncated .mo string")?;
        Ok(String::from_utf8_lossy(slice).into_owned())
    };
    (0..count)
        .map(|i| Ok((string(originals, i)?, string(translations, i)?)))
        .collect()
}

fn wanted(english: &str) -> bool {
    WORDS.contains(&english) || PREFIXES.iter().any(|p| english.starts_with(p))
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
}

/// Reads the catalogs out of the `postgres:<major>` images and writes the table to `out`.
///
/// # Errors
///
/// Fails if Docker fails, an image lacks a catalog, or the file cannot be written.
pub fn extract(out: &Path) -> anyhow::Result<usize> {
    let scratch = std::env::temp_dir().join(format!("pgloupe-locales-{}", std::process::id()));
    fs::create_dir_all(&scratch)?;
    let mut rows: BTreeSet<(String, String, String)> = BTreeSet::new();
    for major in MAJORS {
        let image = format!("postgres:{major}");
        let name = format!("pgloupe-locales-{}-{major}", std::process::id());
        docker(&["create", "--name", name.as_str(), image.as_str()])?;
        let result = (|| -> anyhow::Result<()> {
            for language in LANGUAGES {
                let source =
                    format!("{name}:/usr/share/locale/{language}/LC_MESSAGES/postgres-{major}.mo");
                let target = scratch.join(format!("{language}-{major}.mo"));
                let target_text = target.to_string_lossy().into_owned();
                // A language without enough translated strings has no catalog in that release.
                if docker(&["cp", source.as_str(), target_text.as_str()]).is_err() {
                    eprintln!("note: no {language} catalog in PostgreSQL {major}");
                    continue;
                }
                for (english, translated) in read_mo(&fs::read(&target)?)? {
                    if wanted(&english) && !translated.is_empty() && translated != english {
                        rows.insert(((*language).to_owned(), english, translated));
                    }
                }
            }
            Ok(())
        })();
        docker(&["rm", "-f", name.as_str()])?;
        result?;
    }
    drop(fs::remove_dir_all(&scratch));
    let mut text = String::from(
        "# Generated by `cargo run -p synth -- extract-locales` from the message catalogs of\n\
         # PostgreSQL 14-18 (PostgreSQL License, Copyright PostgreSQL Global Development Group).\n\
         # locale<TAB>english<TAB>translated\n",
    );
    for (language, english, translated) in &rows {
        writeln!(
            text,
            "{language}\t{}\t{}",
            escape(english),
            escape(translated)
        )?;
    }
    fs::write(out, text).with_context(|| format!("writing {}", out.display()))?;
    Ok(rows.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-entry little-endian catalog.
    fn catalog(english: &str, translated: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        let words = |n: u32| n.to_le_bytes();
        let strings_at = 28 + 16;
        for w in [0x9504_12de_u32, 0, 1, 28, 36, 0, 0] {
            bytes.extend(words(w));
        }
        let (o, t) = (english.len(), translated.len());
        bytes.extend(words(u32::try_from(o).unwrap()));
        bytes.extend(words(u32::try_from(strings_at).unwrap()));
        bytes.extend(words(u32::try_from(t).unwrap()));
        bytes.extend(words(u32::try_from(strings_at + o + 1).unwrap()));
        bytes.extend(english.as_bytes());
        bytes.push(0);
        bytes.extend(translated.as_bytes());
        bytes.push(0);
        bytes
    }

    #[test]
    fn a_catalog_is_read_and_only_wanted_messages_are_kept() {
        let pairs = read_mo(&catalog("duration: %s ms", "duração: %s ms")).unwrap();
        assert_eq!(
            pairs,
            [("duration: %s ms".to_owned(), "duração: %s ms".to_owned())]
        );
        assert!(wanted("duration: %s ms"));
        assert!(wanted("ERROR"));
        assert!(!wanted("relation \"%s\" does not exist"));
        read_mo(b"not a catalog at all, really").unwrap_err();
    }

    #[test]
    fn newlines_and_tabs_survive_the_table() {
        assert_eq!(escape("a\nb\tc\\"), "a\\nb\\tc\\\\");
    }
}
