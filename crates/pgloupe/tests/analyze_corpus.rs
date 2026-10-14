//! Checks against the synthetic corpus. Needs `cargo run -p synth -- generate`.

use std::{collections::BTreeMap, io, path::PathBuf, time::Instant};

use pgloupe::{
    analyze::{
        event::{Event, classify},
        pipeline::{self, Options},
    },
    log::{Format, Options as LogOptions, decoder_for, source},
};

const PREFIX: &str = "%m [%p] %q%u@%d/%a %Q ";

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/synth/17-42")
}

fn io_err(err: impl std::fmt::Display) -> io::Error {
    io::Error::other(err.to_string())
}

/// Event counts per (application, event kind) for one log file, plus the number of explicit
/// `CHECKPOINT` commands (checkpoints come from the checkpointer, which has no application).
/// Event counts keyed by (application, event kind).
type Counts = BTreeMap<(String, String), u64>;

fn counts(file: &str, format: Format) -> io::Result<(Counts, u64)> {
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
    let csv = run("postgresql.csv", None)?;
    let json = run("postgresql.json", None)?;
    let stderr = run("postgresql.log", Some(PREFIX))?;
    assert_eq!(csv.acc.totals.records, 1741);
    assert_eq!(csv.acc.totals.events, json.acc.totals.events);
    assert_eq!(csv.acc.totals.events, stderr.acc.totals.events);
    assert_eq!(csv.acc.queries.len(), json.acc.queries.len());
    assert_eq!(csv.acc.queries.len(), stderr.acc.queries.len());
    let statements =
        |o: &pipeline::Output| -> u64 { o.acc.queries.values().map(|q| q.calls).sum() };
    assert_eq!(statements(&csv), statements(&json));
    assert_eq!(statements(&csv), statements(&stderr));
    Ok(())
}

#[test]
#[ignore = "needs data/synth/17-42 and prints timings"]
fn end_to_end_speed_on_the_repeated_corpus() -> io::Result<()> {
    let one = std::fs::read(corpus().join("log/postgresql.json"))?;
    let path = std::env::temp_dir().join(format!("pgloupe-perf-{}.json", std::process::id()));
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
