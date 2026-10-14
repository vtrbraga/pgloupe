//! Store checks against the synthetic corpus. Needs `cargo run -p synth -- generate`.

use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::Instant,
};

use serde_json::Value;

const PREFIX: &str = "%m [%p] %q%u@%d/%a %Q ";

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/synth/17-42")
}

fn pgloupe(args: &[&str]) -> io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_pgloupe"))
        .args(args)
        .output()
}

fn s(path: &Path) -> &str {
    path.to_str().unwrap_or_default()
}

fn comparable(output: &Output) -> io::Result<Value> {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut value: Value = serde_json::from_slice(&output.stdout)?;
    if let Some(map) = value.as_object_mut() {
        for key in ["files", "diagnostics", "store"] {
            map.remove(key);
        }
    }
    Ok(value)
}

fn first_word(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_owned()
}

#[test]
#[ignore = "needs data/synth/17-42 (cargo run -p synth -- generate)"]
fn the_store_agrees_with_analyze_in_every_format() -> io::Result<()> {
    for (file, extra) in [
        ("postgresql.csv", vec![]),
        ("postgresql.json", vec![]),
        ("postgresql.log", vec!["--prefix", PREFIX]),
    ] {
        let log = corpus().join("log").join(file);
        let store =
            std::env::temp_dir().join(format!("pgloupe-corpus-{}-{file}.db", std::process::id()));
        let _ = fs::remove_file(&store);
        let mut ingest = vec!["logs", "ingest", "--store", s(&store)];
        ingest.extend(&extra);
        ingest.push(s(&log));
        assert_eq!(first_word(&pgloupe(&ingest)?), "ingested", "{file}");
        assert_eq!(first_word(&pgloupe(&ingest)?), "unchanged", "{file}");
        let stored = pgloupe(&["logs", "report", "--store", s(&store), "--output", "json"])?;
        let mut analyze = vec!["logs", "analyze", "--output", "json"];
        analyze.extend(&extra);
        analyze.push(s(&log));
        assert_eq!(
            comparable(&stored)?,
            comparable(&pgloupe(&analyze)?)?,
            "{file}"
        );
        fs::remove_file(&store)?;
    }
    Ok(())
}

#[test]
#[ignore = "needs data/synth/17-42 and prints timings; run with --release"]
fn ingest_and_report_speed_on_the_repeated_corpus() -> io::Result<()> {
    let one = fs::read(corpus().join("log/postgresql.json"))?;
    let log = std::env::temp_dir().join(format!("pgloupe-store-perf-{}.json", std::process::id()));
    let store = std::env::temp_dir().join(format!("pgloupe-store-perf-{}.db", std::process::id()));
    let _ = fs::remove_file(&store);
    let mut big = Vec::with_capacity(one.len() * 100);
    for _ in 0..100 {
        big.extend_from_slice(&one);
    }
    fs::write(&log, &big)?;
    let started = Instant::now();
    let out = pgloupe(&["logs", "ingest", "--store", s(&store), s(&log)])?;
    assert!(out.status.success());
    let ingest = started.elapsed().as_secs_f64();
    let started = Instant::now();
    let out = pgloupe(&["logs", "report", "--store", s(&store), "--output", "json"])?;
    assert!(out.status.success());
    let report = started.elapsed().as_secs_f64();
    let started = Instant::now();
    let out = pgloupe(&["logs", "ingest", "--store", s(&store), s(&log)])?;
    assert_eq!(first_word(&out), "unchanged");
    let again = started.elapsed().as_secs_f64();
    let size = fs::metadata(&store)?.len();
    println!(
        "input {} B: ingest {ingest:.2}s, report {report:.2}s, re-ingest {again:.3}s, store {size} B",
        big.len()
    );
    fs::remove_file(&log)?;
    fs::remove_file(&store)?;
    Ok(())
}
