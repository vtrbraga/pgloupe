//! End-to-end tests of `pgloupe report`.

use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use serde_json::Value;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn dir(name: &str) -> io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("pgloupe-combined-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn pgloupe(args: &[&str]) -> io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_pgloupe"))
        .args(args)
        .output()
}

fn s(path: &Path) -> &str {
    path.to_str().unwrap_or_default()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn logs() -> PathBuf {
    fixtures().join("combined/logs.json")
}

fn snapshots() -> (PathBuf, PathBuf) {
    (
        fixtures().join("stats/basic_before.csv"),
        fixtures().join("stats/basic_after.csv"),
    )
}

fn json(args: &[&str]) -> io::Result<Value> {
    let mut full = vec!["report", "--output", "json"];
    full.extend(args);
    let out = pgloupe(&full)?;
    assert!(out.status.success(), "{}", stderr(&out));
    Ok(serde_json::from_str(&stdout(&out))?)
}

fn links(report: &Value) -> Vec<&str> {
    report["statements"]
        .as_array()
        .map(|rows| rows.iter().filter_map(|r| r["link"].as_str()).collect())
        .unwrap_or_default()
}

#[test]
fn logs_only_report_in_every_format() -> io::Result<()> {
    let log = logs();
    let report = json(&[s(&log)])?;
    assert_eq!(report["kind"], "pgloupe.report");
    assert_eq!(report["schema_version"], 1);
    assert!(links(&report).iter().all(|l| *l == "log"));
    assert!(report["inputs"]["stats"].is_null());
    let text = pgloupe(&["report", s(&log)])?;
    assert!(text.status.success(), "{}", stderr(&text));
    assert!(
        stdout(&text).starts_with("pgloupe report\n"),
        "{}",
        stdout(&text)
    );
    let html = pgloupe(&["report", s(&log), "--output", "html"])?;
    assert!(html.status.success(), "{}", stderr(&html));
    let page = stdout(&html);
    assert!(page.starts_with("<!doctype html>"));
    assert!(page.contains("UPDATE film SET rental_rate = $1"));
    Ok(())
}

#[test]
fn logs_and_snapshots_join_per_statement() -> io::Result<()> {
    let (log, (before, after)) = (logs(), snapshots());
    let report = json(&[s(&log), "--before", s(&before), "--after", s(&after)])?;
    let rows = report["statements"].as_array().unwrap();
    let find = |needle: &str| {
        rows.iter()
            .find(|r| r["text"].as_str().unwrap().contains(needle))
    };
    let film = find("film_id").unwrap();
    assert_eq!(film["link"], "both");
    assert!(film["coverage"].as_f64().unwrap() > 0.0 && film["coverage"].as_f64().unwrap() < 1.0);
    assert_eq!(find("DELETE FROM rental").unwrap()["link"], "stats");
    assert_eq!(find("FROM actor").unwrap()["link"], "log");
    let codes: Vec<&str> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["code"].as_str())
        .collect();
    assert!(codes.contains(&"stats_blind_spot"), "{codes:?}");
    assert!(report["windows"]["overlap_pct"].as_f64().unwrap() > 99.0);
    Ok(())
}

#[test]
fn a_store_source_uses_the_snapshot_window() -> io::Result<()> {
    let d = dir("store")?;
    let (log, (before, after)) = (logs(), snapshots());
    let store = d.join("s.db");
    let ingested = pgloupe(&["logs", "ingest", "--store", s(&store), s(&log)])?;
    assert!(ingested.status.success(), "{}", stderr(&ingested));
    let report = json(&[
        "--store",
        s(&store),
        "--before",
        s(&before),
        "--after",
        s(&after),
    ])?;
    assert_eq!(report["inputs"]["logs"]["source"], "store");
    assert!(report["windows"]["logs"].is_object() && report["windows"]["stats"].is_object());
    // The snapshots cover 10:00-10:10, so the store window is the whole of hour 10.
    assert_eq!(report["logs"]["store"]["since"], "2026-10-14T10:00:00Z");
    assert_eq!(report["logs"]["store"]["until"], "2026-10-14T11:00:00Z");
    assert!(links(&report).contains(&"both"));
    Ok(())
}

#[test]
fn a_snapshots_only_report_works() -> io::Result<()> {
    let (before, after) = snapshots();
    let report = json(&["--before", s(&before), "--after", s(&after)])?;
    assert!(report["logs"].is_null() && report["inputs"]["logs"].is_null());
    assert!(links(&report).iter().all(|l| *l == "stats"));
    let text = pgloupe(&["report", "--before", s(&before), "--after", s(&after)])?;
    assert!(
        stdout(&text).contains("logs:      none"),
        "{}",
        stdout(&text)
    );
    Ok(())
}

#[test]
fn html_goes_to_the_out_file() -> io::Result<()> {
    let d = dir("out")?;
    let log = logs();
    let file = d.join("report.html");
    let out = pgloupe(&["report", s(&log), "--output", "html", "--out", s(&file)])?;
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
    let page = fs::read_to_string(&file)?;
    assert!(page.starts_with("<!doctype html>"));
    assert!(!page.contains("http://") && !page.contains("https://"));
    Ok(())
}

#[test]
fn bad_combinations_are_usage_errors() -> io::Result<()> {
    let d = dir("usage")?;
    let (log, (before, after)) = (logs(), snapshots());
    let store = d.join("s.db");
    for args in [
        vec!["report"],
        vec!["report", s(&log), "--store", s(&store)],
        vec!["report", "--before", s(&before)],
        vec!["report", "--after", s(&after)],
        vec!["report", s(&log), "--since", "2026-10-14"],
        vec!["report", s(&log), "--before-info", s(&before)],
    ] {
        let out = pgloupe(&args)?;
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", stderr(&out));
    }
    Ok(())
}

#[test]
fn data_problems_exit_one_with_a_message() -> io::Result<()> {
    let d = dir("data")?;
    let (log, (before, after)) = (logs(), snapshots());
    let garbage = d.join("garbage.csv");
    fs::write(&garbage, "not,a,snapshot\n1,2,3\n")?;
    let out = pgloupe(&[
        "report",
        s(&log),
        "--before",
        s(&garbage),
        "--after",
        s(&after),
    ])?;
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_ne!(stderr(&out), "");
    let one_info = pgloupe(&[
        "report",
        s(&log),
        "--before",
        s(&before),
        "--after",
        s(&after),
        "--before-info",
        s(&fixtures().join("stats/reset_before_info.csv")),
    ])?;
    assert_eq!(one_info.status.code(), Some(2));
    assert!(
        stderr(&one_info).contains("must be given together"),
        "{}",
        stderr(&one_info)
    );
    let missing = pgloupe(&["report", "--store", s(&d.join("nope.db"))])?;
    assert_eq!(missing.status.code(), Some(1));
    assert!(
        stderr(&missing).contains("no such store"),
        "{}",
        stderr(&missing)
    );
    Ok(())
}

#[test]
fn logs_that_decode_to_nothing_still_report_but_fail() -> io::Result<()> {
    let d = dir("nothing")?;
    let junk = d.join("junk.csv");
    fs::write(&junk, "this is not a log\nneither is this\n")?;
    let out = pgloupe(&["report", s(&junk), "--format", "json", "--output", "json"])?;
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let report: Value = serde_json::from_str(&stdout(&out))?;
    assert_eq!(report["kind"], "pgloupe.report");
    Ok(())
}

#[test]
fn the_text_report_says_what_to_fix_first() -> io::Result<()> {
    let line = |second: u32, severity: &str, message: &str, extra: &str| {
        format!(
            r#"{{"timestamp":"2026-10-14 10:00:{second:02}.000 UTC","pid":7,"session_id":"a.1","line_num":{second},"error_severity":"{severity}","message":"{message}"{extra},"backend_type":"client backend"}}"#
        ) + "\n"
    };
    let log = line(1, "LOG", "duration: 5 ms  statement: SELECT 1 FROM a", "")
        + &line(2, "LOG", "duration: 5 ms  statement: SELECT 1 FROM b", "")
        + &line(3, "LOG", "duration: 5 ms  statement: SELECT 1 FROM c", "")
        + &line(
            4,
            "ERROR",
            "deadlock detected",
            r#","state_code":"40P01","statement":"UPDATE t SET a = 1","detail":"Process 7 waits for ShareLock on transaction 9""#,
        );
    let path = std::env::temp_dir().join(format!("pgloupe-actions-{}.json", std::process::id()));
    std::fs::write(&path, log)?;
    let out = pgloupe(&["report", s(&path)])?;
    assert!(out.status.success(), "{}", stderr(&out));
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let at = text
        .find("what to fix first")
        .expect("a what-to-fix-first block");
    let block = &text[at..];
    assert!(block.contains("1. [high] 1 deadlock"), "{block}");
    assert!(block.contains("same order"), "{block}");
    assert!(block.contains("ShareLock"), "{block}");
    Ok(())
}
