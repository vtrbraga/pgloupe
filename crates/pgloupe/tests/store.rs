//! End-to-end tests of `pgloupe logs ingest` and `pgloupe logs report`.

use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Output},
};

use serde_json::Value;

const PREFIX: &str = "%m [%p] %q%u@%d/%a %Q ";

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn dir(name: &str) -> io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("pgloupe-store-e2e-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn pgloupe(args: &[&str]) -> io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_pgloupe"))
        .args(args)
        .output()
}

fn path_str(path: &Path) -> &str {
    path.to_str().unwrap_or_default()
}

fn ingest(store: &Path, files: &[&Path], extra: &[&str]) -> io::Result<Output> {
    let mut args = vec!["logs", "ingest", "--store", path_str(store)];
    args.extend(extra);
    args.extend(files.iter().map(|f| path_str(f)));
    pgloupe(&args)
}

fn report(store: &Path, extra: &[&str]) -> io::Result<Output> {
    let mut args = vec![
        "logs",
        "report",
        "--store",
        path_str(store),
        "--output",
        "json",
    ];
    args.extend(extra);
    pgloupe(&args)
}

fn analyze(files: &[&Path], extra: &[&str]) -> io::Result<Output> {
    let mut args = vec!["logs", "analyze", "--output", "json"];
    args.extend(extra);
    args.extend(files.iter().map(|f| path_str(f)));
    pgloupe(&args)
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The report minus the sections that legitimately differ between `analyze` and `report`.
fn comparable(output: &Output) -> io::Result<Value> {
    assert!(output.status.success(), "{}", stderr(output));
    let mut value: Value = serde_json::from_str(&stdout(output))?;
    if let Some(map) = value.as_object_mut() {
        for key in ["files", "diagnostics", "store"] {
            map.remove(key);
        }
    }
    Ok(value)
}

fn line(hour: usize, minute: usize, ms: usize, sql: &str) -> String {
    format!(
        r#"{{"timestamp":"2026-10-14 {hour:02}:{minute:02}:00.000 UTC","pid":1,"session_id":"a.1","line_num":1,"error_severity":"LOG","message":"duration: {ms}.0 ms  statement: {sql}","backend_type":"client backend"}}"#
    ) + "\n"
}

/// Five statements in each of three hours; hour `h` runs `SELECT * FROM t{h}`.
fn three_hours() -> String {
    let mut text = String::new();
    for hour in 10..13 {
        for minute in 0..5 {
            text.push_str(&line(
                hour,
                minute,
                1 + minute,
                &format!("SELECT * FROM t{hour}"),
            ));
        }
    }
    text
}

fn gz(bytes: &[u8]) -> io::Result<Vec<u8>> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes)?;
    encoder.finish()
}

#[test]
fn ingest_twice_says_unchanged() -> io::Result<()> {
    let d = dir("twice")?;
    let store = d.join("s.db");
    let log = fixtures().join("logs/sample.json");
    let first = ingest(&store, &[&log], &[])?;
    assert!(first.status.success(), "{}", stderr(&first));
    assert!(
        stdout(&first).starts_with("ingested "),
        "{}",
        stdout(&first)
    );
    let before = comparable(&report(&store, &[])?)?;
    let second = ingest(&store, &[&log], &[])?;
    assert!(second.status.success(), "{}", stderr(&second));
    assert!(
        stdout(&second).starts_with("unchanged "),
        "{}",
        stdout(&second)
    );
    assert_eq!(comparable(&report(&store, &[])?)?, before);
    Ok(())
}

#[test]
fn report_equals_analyze_for_every_format() -> io::Result<()> {
    for (name, extra) in [
        ("logs/sample.csv", vec![]),
        ("logs/sample.json", vec![]),
        ("logs/sample.log", vec!["--prefix", PREFIX]),
    ] {
        let d = dir(&format!("equal-{}", name.replace('/', "-")))?;
        let store = d.join("s.db");
        let log = fixtures().join(name);
        let stored = ingest(&store, &[&log], &extra)?;
        assert!(stored.status.success(), "{name}: {}", stderr(&stored));
        assert_eq!(
            comparable(&report(&store, &[])?)?,
            comparable(&analyze(&[&log], &extra)?)?,
            "{name}"
        );
    }
    Ok(())
}

#[test]
fn rotation_by_rename_and_gzip_is_recognised() -> io::Result<()> {
    let d = dir("rotation")?;
    let store = d.join("s.db");
    let text = three_hours();
    let a = d.join("postgresql.json");
    fs::write(&a, &text)?;
    assert!(stdout(&ingest(&store, &[&a], &[])?).starts_with("ingested"));
    let renamed = d.join("postgresql.json.1");
    fs::rename(&a, &renamed)?;
    let compressed = d.join("postgresql.json.2.gz");
    fs::write(&compressed, gz(text.as_bytes())?)?;
    let out = ingest(&store, &[&renamed, &compressed], &[])?;
    let lines = stdout(&out);
    assert_eq!(lines.lines().count(), 2, "{lines}");
    assert!(lines.lines().all(|l| l.starts_with("unchanged")), "{lines}");
    assert_eq!(
        comparable(&report(&store, &[])?)?["overview"]["records"],
        15
    );
    Ok(())
}

#[test]
fn growth_replaces_and_matches_fresh_ingest() -> io::Result<()> {
    let d = dir("growth")?;
    let full = three_hours();
    let log = d.join("a.json");
    let grown = d.join("grown.db");
    fs::write(&log, &full[..full.len() - 40])?;
    assert!(stdout(&ingest(&grown, &[&log], &[])?).starts_with("ingested"));
    fs::write(&log, &full)?;
    let out = ingest(&grown, &[&log], &[])?;
    assert!(stdout(&out).starts_with("replaced"), "{}", stdout(&out));
    let fresh = d.join("fresh.db");
    let copy = d.join("b.json");
    fs::write(&copy, &full)?;
    ingest(&fresh, &[&copy], &[])?;
    assert_eq!(
        comparable(&report(&grown, &[])?)?,
        comparable(&report(&fresh, &[])?)?
    );
    Ok(())
}

#[test]
fn window_equals_analyze_of_the_filtered_log() -> io::Result<()> {
    let d = dir("window")?;
    let store = d.join("s.db");
    let log = d.join("a.json");
    fs::write(&log, three_hours())?;
    ingest(&store, &[&log], &[])?;
    let only_11 = d.join("only11.json");
    let filtered: String = three_hours()
        .lines()
        .filter(|l| l.contains(" 11:"))
        .flat_map(|l| [l, "\n"])
        .collect();
    fs::write(&only_11, filtered)?;
    let windowed = report(
        &store,
        &[
            "--since",
            "2026-10-14T11:00:00Z",
            "--until",
            "2026-10-14T12:00:00Z",
        ],
    )?;
    assert_eq!(
        comparable(&windowed)?,
        comparable(&analyze(&[&only_11], &[])?)?
    );
    Ok(())
}

#[test]
fn unsnapped_times_are_snapped_and_reported() -> io::Result<()> {
    let d = dir("snap")?;
    let store = d.join("s.db");
    let log = d.join("a.json");
    fs::write(&log, three_hours())?;
    ingest(&store, &[&log], &[])?;
    let out = pgloupe(&[
        "logs",
        "report",
        "--store",
        path_str(&store),
        "--since",
        "2026-10-14T11:20:00Z",
        "--until",
        "2026-10-14T11:40:00Z",
    ])?;
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with("pgloupe logs report\n"), "{text}");
    assert!(
        text.contains("range:      2026-10-14T11:00:00Z -> 2026-10-14T12:00:00Z"),
        "{text}"
    );
    Ok(())
}

#[test]
fn bad_store_and_bad_window_fail_clearly() -> io::Result<()> {
    let d = dir("errors")?;
    let garbage = d.join("garbage.db");
    let bytes = b"definitely not a database, just some text that is long enough....";
    fs::write(&garbage, bytes)?;
    let out = report(&garbage, &[])?;
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("not a pgloupe store"),
        "{}",
        stderr(&out)
    );
    let out = ingest(&garbage, &[&fixtures().join("logs/sample.json")], &[])?;
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(fs::read(&garbage)?, bytes);

    let missing = d.join("missing.db");
    let out = report(&missing, &[])?;
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("no such store"), "{}", stderr(&out));
    assert!(!missing.exists());

    let store = d.join("s.db");
    ingest(&store, &[&fixtures().join("logs/sample.json")], &[])?;
    let out = report(
        &store,
        &[
            "--since",
            "2026-10-14T12:00:00Z",
            "--until",
            "2026-10-14T11:00:00Z",
        ],
    )?;
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("--since must be earlier"),
        "{}",
        stderr(&out)
    );
    let out = report(&store, &["--bucket", "30s"])?;
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("one-minute buckets"),
        "{}",
        stderr(&out)
    );
    Ok(())
}

#[test]
fn threads_do_not_change_the_report() -> io::Result<()> {
    let d = dir("threads")?;
    let log = d.join("a.json");
    let mut text = String::new();
    for _ in 0..40 {
        text.push_str(&three_hours());
    }
    fs::write(&log, text)?;
    let mut reports = Vec::new();
    for threads in ["1", "4"] {
        let store = d.join(format!("t{threads}.db"));
        let out = ingest(&store, &[&log], &["--threads", threads])?;
        assert!(out.status.success(), "{}", stderr(&out));
        reports.push(comparable(&report(&store, &[])?)?);
    }
    assert_eq!(reports[0], reports[1]);
    Ok(())
}

#[test]
fn a_failed_file_does_not_block_the_others() -> io::Result<()> {
    let d = dir("partial")?;
    let store = d.join("s.db");
    let bad = d.join("bad.log");
    fs::write(&bad, [0u8; 64])?;
    let good = fixtures().join("logs/sample.json");
    let out = ingest(&store, &[&bad, &good], &[])?;
    assert_eq!(out.status.code(), Some(1));
    let lines = stdout(&out);
    assert!(
        lines.lines().next().unwrap_or("").starts_with("failed"),
        "{lines}"
    );
    assert!(
        lines.lines().nth(1).unwrap_or("").starts_with("ingested"),
        "{lines}"
    );
    assert!(report(&store, &[])?.status.success());
    Ok(())
}

#[test]
fn force_reingests() -> io::Result<()> {
    let d = dir("force")?;
    let store = d.join("s.db");
    let log = fixtures().join("logs/sample.json");
    ingest(&store, &[&log], &[])?;
    let out = ingest(&store, &[&log], &["--force"])?;
    assert!(stdout(&out).starts_with("replaced "), "{}", stdout(&out));
    Ok(())
}
