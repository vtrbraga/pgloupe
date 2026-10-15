//! Cross-format equivalence on the synthetic corpus. Needs `cargo run -p synth -- generate`.

use std::{io, path::PathBuf};

use pgloupe::log::{
    DecodeStats, Format, LogRecord, Options, decoder_for, locale::Translator, source,
};

const PREFIX: &str = "%m [%p] %q%u@%d/%a %Q ";

/// Whether the corpus has this log (Postgres 14 has no jsonlog).
fn present(name: &str) -> bool {
    corpus_dir().join(name).exists()
}

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../data/synth/{}-42/log",
        std::env::var("PGLOUPE_PG").unwrap_or_else(|_| "17".to_owned())
    ))
}

fn decode(name: &str, format: Format) -> io::Result<(Vec<LogRecord>, DecodeStats)> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!(
            "../../data/synth/{}-42/log",
            std::env::var("PGLOUPE_PG").unwrap_or_else(|_| "17".to_owned())
        ))
        .join(name);
    let reader = source::open(&path)?;
    let options = Options {
        prefix: Some(PREFIX.to_owned()),
        log_tz: None,
    };
    let mut decoder =
        decoder_for(format, reader, &options).map_err(|err| io::Error::other(err.to_string()))?;
    let mut records = Vec::new();
    while let Some(record) = decoder
        .next_record()
        .map_err(|err| io::Error::other(err.to_string()))?
    {
        records.push(record);
    }
    Ok((records, decoder.stats().clone()))
}

/// The events rendered with `render`, sorted.
fn sorted(records: &[LogRecord], render: impl Fn(&LogRecord) -> String) -> Vec<String> {
    let mut lines: Vec<String> = records.iter().map(render).collect();
    lines.sort();
    lines
}

/// Fails with the first events that exist on one side only.
fn assert_same(left: &[String], right: &[String], what: &str) {
    let only_left: Vec<&String> = left.iter().filter(|l| !right.contains(l)).take(3).collect();
    let only_right: Vec<&String> = right.iter().filter(|r| !left.contains(r)).take(3).collect();
    assert!(
        only_left.is_empty() && only_right.is_empty() && left.len() == right.len(),
        "{what}: {} vs {} events\nonly left: {only_left:#?}\nonly right: {only_right:#?}",
        left.len(),
        right.len()
    );
}

#[test]
#[ignore = "needs data/synth/17-42 (cargo run -p synth -- generate)"]
fn the_three_formats_decode_to_the_same_events() -> io::Result<()> {
    let (csv, csv_stats) = decode("postgresql.csv", Format::Csv)?;
    let json = present("postgresql.json")
        .then(|| decode("postgresql.json", Format::Json))
        .transpose()?;
    let (mut stderr, stderr_stats) = decode("postgresql.log", Format::Stderr)?;
    // The decoder splits ` at character N` off English errors only; the analysis does the rest.
    let translators: Vec<Translator> = ["pt_BR", "es", "fr", "de"]
        .iter()
        .filter_map(|locale| Translator::new(locale, None).ok())
        .collect();
    for record in &mut stderr {
        if record.cursor_position.is_some() {
            continue;
        }
        // `LOG` lines keep the English suffix in the decoder (lock waits end that way).
        let english = record
            .message
            .rsplit_once(" at character ")
            .and_then(|(message, n)| Some((message.to_owned(), n.parse().ok()?)));
        let split = english.or_else(|| {
            translators.iter().find_map(|t| {
                t.split_position(&record.message)
                    .map(|(message, position)| (message.to_owned(), position))
            })
        });
        if let Some((message, position)) = split {
            record.message = message;
            record.cursor_position = Some(position);
        }
    }

    for (name, stats) in [("csv", &csv_stats), ("stderr", &stderr_stats)] {
        assert_eq!(stats.skipped, 0, "{name}: {:?}", stats.first_skipped);
    }
    if let Some((_, stats)) = &json {
        assert_eq!(stats.skipped, 0, "json: {:?}", stats.first_skipped);
    }
    assert!(csv.len() > 1_000, "{} records", csv.len());
    assert_eq!(csv.len(), stderr.len());

    // Concurrent backends log in the same millisecond, and each format is its own log stream,
    // so the order of such lines differs between files: compare the events as sorted sets.
    // csv and json carry the same fields.
    if let Some((json, _)) = &json {
        assert_eq!(csv.len(), json.len());
        assert_same(
            &sorted(&csv, |r| format!("{r:?}")),
            &sorted(json, |r| format!("{r:?}")),
            "csv vs json",
        );
    }

    // stderr carries only what the prefix has: compare those fields with csv.
    // Temporary-file lines from a parallel worker carry the session's names in csvlog but not
    // in stderr (the prefix stops at `%q` for them), and which one it is depends on timing.
    let shared = |r: &LogRecord| {
        let worker = r.message.starts_with("temporary file:");
        let named = |value: &Option<String>| if worker { None } else { value.clone() };
        format!(
            "{:?} {:?}",
            (
                r.ts,
                r.level,
                &r.message,
                r.pid,
                named(&r.user),
                named(&r.database),
                named(&r.application),
            ),
            (
                r.query_id.filter(|_| !worker),
                &r.detail,
                &r.hint,
                &r.context,
                &r.statement,
                r.cursor_position,
            )
        )
    };
    assert_same(
        &sorted(&csv, shared),
        &sorted(&stderr, shared),
        "csv vs stderr",
    );
    Ok(())
}
