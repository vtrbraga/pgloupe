//! The combined report on the synthetic corpus. Needs `cargo run -p synth -- generate`.

use std::{
    collections::BTreeSet,
    io,
    path::PathBuf,
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

fn report(log: &str, extra: &[&str]) -> io::Result<Value> {
    let d = corpus();
    let path = |name: &str| d.join(name).display().to_string();
    let (log, b, a, bi, ai) = (
        path(&format!("log/{log}")),
        path("pgss_0.csv"),
        path("pgss_1.csv"),
        path("pgss_info_0.csv"),
        path("pgss_info_1.csv"),
    );
    let mut args = vec![
        "report",
        &log,
        "--before",
        &b,
        "--after",
        &a,
        "--before-info",
        &bi,
        "--after-info",
        &ai,
        "--output",
        "json",
        "--top",
        "500",
    ];
    args.extend(extra);
    let out = pgloupe(&args)?;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(serde_json::from_slice(&out.stdout)?)
}

/// `(fingerprint, link, statistics calls)` of every statement row.
fn shape(report: &Value) -> BTreeSet<(String, String, u64)> {
    report["statements"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            Some((
                row["fingerprint"].as_str()?.to_owned(),
                row["link"].as_str()?.to_owned(),
                row["stats"]["calls"].as_u64().unwrap_or(0),
            ))
        })
        .collect()
}

#[test]
#[ignore = "needs data/synth/17-42 (cargo run -p synth -- generate)"]
fn the_corpus_logs_join_with_their_snapshots_in_every_format() -> io::Result<()> {
    let json = report("postgresql.json", &[])?;
    let csv = report("postgresql.csv", &[])?;
    let stderr = report("postgresql.log", &["--prefix", PREFIX])?;
    let rows = json["statements"].as_array().unwrap();
    let both: Vec<&Value> = rows.iter().filter(|r| r["link"] == "both").collect();
    assert!(both.len() >= 10, "only {} joined statements", both.len());
    for row in &both {
        assert_ne!(row["query_id_matches"], Value::Bool(false), "{row}");
        // The corpus logs every statement (log_min_duration_statement = 0), so the log sees
        // the calls the statistics counted.
        let stats_calls = row["stats"]["calls"].as_u64().unwrap();
        if stats_calls >= 20 {
            let coverage = row["coverage"].as_f64().unwrap();
            assert!((0.9..=1.1).contains(&coverage), "{coverage}: {row}");
        }
    }
    assert_eq!(shape(&json), shape(&csv), "csv differs from json");
    assert_eq!(shape(&json), shape(&stderr), "stderr differs from json");
    assert!(json["windows"]["overlap_pct"].as_f64().unwrap() > 90.0);
    Ok(())
}

#[test]
#[ignore = "needs PGLOUPE_BENCH_LOG, PGLOUPE_BENCH_BEFORE and PGLOUPE_BENCH_AFTER; prints timings"]
fn join_and_render_speed_on_a_large_input() -> io::Result<()> {
    let (Some(log), Some(before), Some(after)) = (
        std::env::var_os("PGLOUPE_BENCH_LOG"),
        std::env::var_os("PGLOUPE_BENCH_BEFORE"),
        std::env::var_os("PGLOUPE_BENCH_AFTER"),
    ) else {
        return Ok(());
    };
    let (log, before, after) = (
        log.to_string_lossy().into_owned(),
        before.to_string_lossy().into_owned(),
        after.to_string_lossy().into_owned(),
    );
    for output in ["json", "html"] {
        let started = Instant::now();
        let out = pgloupe(&[
            "report", &log, "--before", &before, "--after", &after, "--output", output,
        ])?;
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        println!(
            "report --output {output}: {:.2}s, {} bytes",
            started.elapsed().as_secs_f64(),
            out.stdout.len()
        );
    }
    Ok(())
}
