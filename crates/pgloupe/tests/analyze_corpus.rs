//! Checks against the synthetic corpus. Needs `cargo run -p synth -- generate`.

use std::{collections::BTreeMap, io, path::PathBuf, time::Instant};

use pgloupe::{
    analyze::{
        event::{Event, classify},
        fingerprint::{fingerprint, fingerprint_uncached},
        pipeline::{self, Options},
    },
    log::{Format, Options as LogOptions, decoder_for, source},
};

const PREFIX: &str = "%m [%p] %q%u@%d/%a %Q ";

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../data/synth/{}-42",
        std::env::var("PGLOUPE_PG").unwrap_or_else(|_| "17".to_owned())
    ))
}

/// Whether the corpus has this log (Postgres 14 has no jsonlog).
fn present(name: &str) -> bool {
    corpus().join("log").join(name).exists()
}

/// The structured log to use when one will do: jsonlog, else csvlog.
fn reference() -> &'static str {
    if present("postgresql.json") {
        "postgresql.json"
    } else {
        "postgresql.csv"
    }
}

fn io_err(err: impl std::fmt::Display) -> io::Error {
    io::Error::other(err.to_string())
}

/// Event counts per (application, event kind) for one log file, plus the number of explicit
/// `CHECKPOINT` commands (checkpoints come from the checkpointer, which has no application).
/// Event counts keyed by (application, event kind).
type Counts = BTreeMap<(String, String), u64>;

/// When the dataset finished loading. Before that the server logs checkpoints of its own (Postgres
/// 14 checkpoints for `CREATE DATABASE`), which no scenario asked for.
fn load_finished_at() -> io::Result<jiff::Timestamp> {
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(corpus().join("manifest.json"))?)?;
    let text = manifest["load_finished_at"].as_str().unwrap_or_default();
    // `2026-10-14 23:23:03.580655+00` -> RFC 3339.
    let rfc3339 = format!("{}:00", text.replacen(' ', "T", 1));
    rfc3339.parse().map_err(io_err)
}

fn counts(file: &str, format: Format) -> io::Result<(Counts, u64)> {
    let loaded = load_finished_at()?;
    let reader = source::open(&corpus().join("log").join(file))?;
    let options = LogOptions {
        prefix: Some(PREFIX.to_owned()),
        log_tz: None,
    };
    let mut decoder = decoder_for(format, reader, &options).map_err(io_err)?;
    let mut counts = BTreeMap::new();
    let mut forced = 0;
    while let Some(record) = decoder.next_record().map_err(io_err)? {
        let event = classify(&record);
        // While a connection is being authorized the session has no application name yet; the
        // log line carries it in its text.
        let app = if matches!(event, Event::ConnectionAuthorized) {
            record
                .message
                .split("application_name=")
                .nth(1)
                .and_then(|rest| rest.split_whitespace().next())
                .unwrap_or_default()
                .to_owned()
        } else {
            record.application.clone().unwrap_or_default()
        };
        if let Event::CheckpointStart { reason, .. } = &event
            && reason == "immediate force wait"
            && record.ts > loaded
        {
            forced += 1;
        }
        *counts.entry((app, event.kind().to_owned())).or_insert(0) += 1;
    }
    Ok((counts, forced))
}

#[test]
#[ignore = "needs data/synth/17-42 (cargo run -p synth -- generate)"]
fn the_manifest_expectations_hold_in_every_format() -> io::Result<()> {
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(corpus().join("manifest.json"))?)?;
    let expectations = manifest["expect"]
        .as_array()
        .ok_or_else(|| io_err("no expect"))?;
    for (file, format) in [
        ("postgresql.csv", Format::Csv),
        ("postgresql.json", Format::Json),
        ("postgresql.log", Format::Stderr),
    ] {
        if !present(file) {
            continue;
        }
        let (counts, forced_checkpoints) = counts(file, format)?;
        for expect in expectations {
            let app = expect["app"].as_str().unwrap_or_default().to_owned();
            let event = expect["event"].as_str().unwrap_or_default();
            // "checkpoint" in the manifest is the explicit CHECKPOINT command, which is not tied to
            // an application; every other event kind has the manifest's name.
            let got = if event == "checkpoint" {
                forced_checkpoints
            } else {
                counts
                    .get(&(app.clone(), event.to_owned()))
                    .copied()
                    .unwrap_or(0)
            };
            let min = expect["min"].as_u64().unwrap_or(0);
            let max = expect["max"].as_u64().unwrap_or(u64::MAX);
            // Errors are separated by SQLSTATE in the manifest; the event kind has no SQLSTATE,
            // so only the total per application is checked for them below.
            if event == "error" {
                continue;
            }
            assert!(
                (min..=max).contains(&got),
                "{file}: {app} {event}: got {got}, expected {min}..={max}"
            );
        }
        let errors_scenario: u64 = counts
            .get(&("synth:errors".to_owned(), "error".to_owned()))
            .copied()
            .unwrap_or(0);
        let expected_errors: u64 = expectations
            .iter()
            .filter(|e| e["event"] == "error")
            .filter_map(|e| e["min"].as_u64())
            .sum();
        assert!(
            errors_scenario >= expected_errors,
            "{file}: errors {errors_scenario} < {expected_errors}"
        );
    }
    Ok(())
}

#[test]
#[ignore = "needs data/synth/17-42 (cargo run -p synth -- generate)"]
fn the_three_formats_give_the_same_report_numbers() -> io::Result<()> {
    let run = |file: &str, prefix: Option<&str>| -> io::Result<pipeline::Output> {
        let options = Options {
            prefix: prefix.map(str::to_owned),
            ..Options::new()
        };
        pipeline::run(&[corpus().join("log").join(file)], &options).map_err(io_err)
    };
    let mut outputs = vec![("csv", run("postgresql.csv", None)?)];
    if present("postgresql.json") {
        outputs.push(("json", run("postgresql.json", None)?));
    }
    outputs.push(("stderr", run("postgresql.log", Some(PREFIX))?));
    let (first_name, first) = &outputs[0];
    assert!(
        first.acc.totals.records > 1_000,
        "{}",
        first.acc.totals.records
    );
    let statements =
        |o: &pipeline::Output| -> u64 { o.acc.queries.values().map(|q| q.calls).sum() };
    for (name, other) in &outputs[1..] {
        assert_eq!(
            first.acc.totals.records, other.acc.totals.records,
            "{first_name} vs {name}"
        );
        assert_eq!(first.acc.totals.events, other.acc.totals.events, "{name}");
        assert_eq!(first.acc.queries.len(), other.acc.queries.len(), "{name}");
        assert_eq!(statements(first), statements(other), "{name}");
    }
    Ok(())
}

#[test]
#[ignore = "needs data/synth/17-42 and prints timings"]
fn end_to_end_speed_on_the_repeated_corpus() -> io::Result<()> {
    let one = std::fs::read(corpus().join("log").join(reference()))?;
    let path = std::env::temp_dir().join(format!("pgloupe-perf-{}.log", std::process::id()));
    let mut big = Vec::with_capacity(one.len() * 100);
    for _ in 0..100 {
        big.extend_from_slice(&one);
    }
    std::fs::write(&path, &big)?;
    for threads in [1, 4, 8] {
        let started = Instant::now();
        let output = pipeline::run(
            std::slice::from_ref(&path),
            &Options {
                threads: Some(threads),
                ..Options::new()
            },
        )
        .map_err(io_err)?;
        let secs = started.elapsed().as_secs_f64();
        #[expect(clippy::cast_precision_loss, reason = "a size in a printed benchmark")]
        let mb = big.len() as f64 / 1_048_576.0;
        println!(
            "threads {threads}: {} records, {} fingerprints, {secs:.2}s, {:.0} MB/s",
            output.acc.totals.records,
            output.acc.queries.len(),
            mb / secs
        );
    }
    std::fs::remove_file(&path)?;
    Ok(())
}

#[test]
#[ignore = "needs data/synth/17-42 (cargo run -p synth -- generate)"]
fn results_do_not_depend_on_threads_or_batches_on_the_real_corpus() -> io::Result<()> {
    let path = corpus().join("log").join(reference());
    let run = |threads: usize, batch_size: usize| {
        pipeline::run(
            std::slice::from_ref(&path),
            &Options {
                threads: Some(threads),
                batch_size,
                ..Options::new()
            },
        )
        .map_err(io_err)
    };
    let reference = run(1, 4_096)?;
    for (threads, batch) in [(8, 1), (3, 17), (8, 4_096)] {
        assert_eq!(
            run(threads, batch)?.acc,
            reference.acc,
            "threads {threads}, batch {batch}"
        );
    }
    Ok(())
}

#[test]
#[ignore = "needs data/synth/17-42 (cargo run -p synth -- generate)"]
fn the_fingerprint_cache_agrees_with_pg_query_on_every_statement() -> io::Result<()> {
    let mut checked = 0;
    for (file, format) in [
        ("postgresql.csv", Format::Csv),
        ("postgresql.json", Format::Json),
        ("postgresql.log", Format::Stderr),
    ] {
        if !present(file) {
            continue;
        }
        let reader = source::open(&corpus().join("log").join(file))?;
        let options = LogOptions {
            prefix: Some(PREFIX.to_owned()),
            log_tz: None,
        };
        let mut decoder = decoder_for(format, reader, &options).map_err(io_err)?;
        // Two passes over the statements, so the second one is served from the cache.
        let mut statements = Vec::new();
        while let Some(record) = decoder.next_record().map_err(io_err)? {
            if let Event::Statement { text, .. } = classify(&record) {
                statements.push(text);
            }
        }
        for pass in 0..2 {
            for text in &statements {
                assert_eq!(
                    fingerprint(text),
                    fingerprint_uncached(text),
                    "{file}, pass {pass}: {text}"
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 1_000, "only {checked} statements checked");
    Ok(())
}
