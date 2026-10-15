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
    assert_eq!(out.status.code(), Some(2));
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

#[test]
fn the_store_keeps_slowest_executions_and_parameters_only_if_ingest_asked() -> io::Result<()> {
    let d = dir("slowest")?;
    let line = |second: u32, ms: u32, detail: &str| {
        format!(
            r#"{{"timestamp":"2026-10-14 10:00:{second:02}.000 UTC","pid":1,"session_id":"a.1","line_num":{second},"error_severity":"LOG","message":"duration: {ms} ms  execute <unnamed>: SELECT 1 FROM t WHERE id = $1","detail":"{detail}","backend_type":"client backend"}}"#
        ) + "\n"
    };
    let log = d.join("a.json");
    fs::write(
        &log,
        line(1, 5, "parameters: $1 = '1'") + &line(2, 80, "parameters: $1 = '2'"),
    )?;
    for (name, extra, expect_params) in
        [("plain", &[][..], false), ("with", &["--params"][..], true)]
    {
        let store = d.join(format!("{name}.db"));
        assert!(ingest(&store, &[&log], extra)?.status.success());
        let out = report(&store, &[])?;
        assert!(out.status.success(), "{}", stderr(&out));
        let value: Value = serde_json::from_str(&stdout(&out))?;
        let slowest = &value["slowest"];
        assert_eq!(slowest["rows"][0]["duration_ms"], 80.0);
        assert_eq!(slowest["has_params"], expect_params);
    }
    Ok(())
}

#[test]
fn a_store_keeps_the_filter_its_logs_were_ingested_with() -> io::Result<()> {
    let d = dir("filter")?;
    let store = d.join("s.db");
    let log = fixtures().join("logs/hours.log");
    let filter = [
        "--prefix",
        "%m [%p] %q%u@%d/%a ",
        "--between",
        "09:00-18:00",
        "--weekdays",
        "mon-fri",
        "--exclude-application",
        "pgAdmin*",
    ];
    let first = ingest(&store, &[&log], &filter)?;
    assert!(first.status.success(), "{}", stderr(&first));
    // Mon and Wed, hours 10 and 14; the 06:00 pgAdmin rows were already outside the hours.
    let value: Value = serde_json::from_str(&stdout(&report(&store, &[])?))?;
    assert_eq!(value["overview"]["events"]["statement"], 4);
    assert!(
        value["store"]["filter"].as_str().is_some_and(
            |f| f.contains("between=09:00-18:00") && f.contains("weekdays=mon,tue,wed,thu,fri")
        ),
        "{value}"
    );
    // The same filter may be used again; another, or none, is refused with the reason.
    assert!(ingest(&store, &[&log], &filter)?.status.success());
    let other = ingest(&store, &[&log], &["--prefix", "%m [%p] %q%u@%d/%a "])?;
    assert!(!other.status.success());
    assert!(stderr(&other).contains("filter"), "{}", stderr(&other));
    // Filters cannot be applied later, to a store report.
    let late = report(&store, &["--user", "u"])?;
    assert!(!late.status.success());
    Ok(())
}

#[test]
fn a_store_report_lists_as_many_slowest_executions_as_asked_up_to_what_was_kept() -> io::Result<()>
{
    let d = dir("slowest-baseline")?;
    let store = d.join("s.db");
    let log = fixtures().join("logs/sample.json");
    assert!(ingest(&store, &[&log], &[])?.status.success());
    let rows = |extra: &[&str]| -> io::Result<usize> {
        let value: Value = serde_json::from_str(&stdout(&report(&store, extra)?))?;
        Ok(value["slowest"]["rows"].as_array().map_or(0, Vec::len))
    };
    let all = rows(&[])?;
    assert!(all >= 2, "the sample has several slow runs ({all})");
    assert_eq!(rows(&["--slowest", "1"])?, 1);
    assert_eq!(rows(&["--slowest", "2"])?, 2.min(all));
    assert_eq!(rows(&["--slowest", "500"])?, all);
    Ok(())
}

/// The store reports in whole hours: 10:00-11:15 reaches 12:00, so it overlaps 11:30-12:00.
fn assert_hour_rounded_baselines_overlap(store: &Path) -> io::Result<()> {
    let rounded = pgloupe(&[
        "report",
        "--store",
        path_str(store),
        "--since",
        "2026-10-14T11:30:00Z",
        "--until",
        "2026-10-14T12:00:00Z",
        "--baseline-since",
        "2026-10-14T10:00:00Z",
        "--baseline-until",
        "2026-10-14T11:15:00Z",
        "--output",
        "json",
    ])?;
    assert!(!rounded.status.success(), "{}", stdout(&rounded));
    assert!(
        stderr(&rounded).contains("overlaps"),
        "{}",
        stderr(&rounded)
    );
    Ok(())
}

#[test]
fn a_period_is_compared_with_the_one_before_it() -> io::Result<()> {
    let d = dir("compare")?;
    let store = d.join("s.db");
    let (before, after) = (
        fixtures().join("logs/compare_before.log"),
        fixtures().join("logs/compare_after.log"),
    );
    let prefix = ["--prefix", "%m [%p] %q%u@%d/%a "];
    assert!(
        ingest(&store, &[&before, &after], &prefix)?
            .status
            .success()
    );
    let compared = |extra: &[&str]| -> io::Result<Output> {
        let mut args = vec![
            "report",
            "--store",
            path_str(&store),
            "--since",
            "2026-10-14T11:00:00Z",
            "--until",
            "2026-10-14T12:00:00Z",
            "--output",
            "json",
        ];
        args.extend(extra);
        pgloupe(&args)
    };
    let out = compared(&["--baseline", "previous"])?;
    assert!(out.status.success(), "{}", stderr(&out));
    let value: Value = serde_json::from_str(&stdout(&out))?;
    let c = &value["comparison"];
    assert_eq!(c["baseline"]["since"], "2026-10-14T10:00:00Z");
    let find = |status: &str| -> Vec<String> {
        c["statements"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|m| m["status"] == status)
            .map(|m| m["text"].as_str().unwrap_or_default().to_owned())
            .collect()
    };
    assert_eq!(find("slower").len(), 1, "{c}");
    assert!(find("slower")[0].contains("orders"));
    assert!(find("new")[0].contains("invoices"));
    assert!(find("gone")[0].contains("legacy"));
    assert_eq!(c["totals"]["errors"]["now"], 1.0);
    assert_eq!(c["totals"]["errors"]["before"], 0.0);
    // An explicit baseline gives the same answer; overlapping or half-given ones are refused.
    let explicit = compared(&[
        "--baseline-since",
        "2026-10-14T10:00:00Z",
        "--baseline-until",
        "2026-10-14T11:00:00Z",
    ])?;
    assert!(explicit.status.success(), "{}", stderr(&explicit));
    let value2: Value = serde_json::from_str(&stdout(&explicit))?;
    assert_eq!(value2["comparison"]["totals"], c["totals"]);
    let overlap = compared(&[
        "--baseline-since",
        "2026-10-14T10:30:00Z",
        "--baseline-until",
        "2026-10-14T11:30:00Z",
    ])?;
    assert!(!overlap.status.success());
    assert_hour_rounded_baselines_overlap(&store)?;
    assert!(
        !compared(&["--baseline-since", "2026-10-14T10:00:00Z"])?
            .status
            .success()
    );
    for (format, needle) in [
        ("text", "comparison (against 2026-10-14T10:00:00Z"),
        ("html", "<h2>Comparison</h2>"),
    ] {
        let out = pgloupe(&[
            "report",
            "--store",
            path_str(&store),
            "--since",
            "2026-10-14T11:00:00Z",
            "--until",
            "2026-10-14T12:00:00Z",
            "--baseline",
            "previous",
            "--output",
            format,
        ])?;
        assert!(out.status.success(), "{}", stderr(&out));
        assert!(stdout(&out).contains(needle), "{format}");
    }
    let plain = compared(&[])?;
    let value3: Value = serde_json::from_str(&stdout(&plain))?;
    assert!(value3.get("comparison").is_none());
    Ok(())
}

#[test]
fn an_anonymized_stores_report_does_not_print_the_names_it_was_filtered_by() -> io::Result<()> {
    let d = dir("anon-filter")?;
    let store = d.join("s.db");
    let log = fixtures().join("logs/sample.json");
    let ingest = pgloupe(&[
        "logs",
        "ingest",
        "--store",
        path_str(&store),
        "--anonymize",
        "--anonymize-key",
        "hunter2",
        "--user",
        "secret_user",
        "--include-query",
        "secret_table",
        path_str(&log),
    ])?;
    assert!(ingest.status.success(), "{}", stderr(&ingest));
    let out = pgloupe(&[
        "report",
        "--store",
        path_str(&store),
        "--anonymize",
        "--anonymize-key",
        "hunter2",
        "--output",
        "json",
    ])?;
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    for secret in ["secret_user", "secret_table", "hunter2", "key "] {
        assert!(!text.contains(secret), "{secret} in the report");
    }
    assert!(text.contains("details withheld"), "{text}");
    Ok(())
}
