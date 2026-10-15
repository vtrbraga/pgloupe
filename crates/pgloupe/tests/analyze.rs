//! End-to-end tests of `pgloupe logs analyze`.

use std::{
    fs, io,
    path::PathBuf,
    process::{Command, Output},
};

const PREFIX: &str = "%m [%p] %q%u@%d/%a %Q ";

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn analyze(args: &[&str], files: &[PathBuf]) -> io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_pgloupe"))
        .args(["logs", "analyze"])
        .args(args)
        .args(files)
        .output()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn scratch(name: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    let path = std::env::temp_dir().join(format!("pgloupe-analyze-{}-{name}", std::process::id()));
    fs::write(&path, bytes)?;
    Ok(path)
}

#[test]
fn text_report_for_the_sample() -> io::Result<()> {
    let out = analyze(&[], &[fixtures().join("logs/sample.json")])?;
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with("pgloupe logs analyze\n"), "{text}");
    for section in [
        "diagnostics",
        "overview",
        "top statements (by total)",
        "errors",
        "locks",
        "temp files",
        "connections",
        "checkpoints",
        "autovacuum",
        "timeline",
    ] {
        assert!(
            text.lines().any(|l| l.starts_with(section)),
            "missing {section} in\n{text}"
        );
    }
    assert!(text.contains("SELECT $1"), "{text}");
    assert!(text.contains("42703"), "{text}");
    Ok(())
}

#[test]
fn json_report_is_versioned() -> io::Result<()> {
    let out = analyze(&["--output", "json"], &[fixtures().join("logs/sample.csv")])?;
    assert!(out.status.success(), "{}", stderr(&out));
    let value: serde_json::Value = serde_json::from_str(&stdout(&out))?;
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["overview"]["records"], 8);
    assert_eq!(value["overview"]["events"]["statement"], 2);
    assert_eq!(value["overview"]["events"]["temp_file"], 1);
    assert_eq!(value["files"][0]["format"], "csv");
    let sqlstates: Vec<&str> = value["errors"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["sqlstate"].as_str())
        .collect();
    assert_eq!(
        sqlstates,
        ["40P01", "42703"],
        "equal counts are ordered by SQLSTATE"
    );
    Ok(())
}

#[test]
fn all_three_formats_give_the_same_overview() -> io::Result<()> {
    let mut overviews = Vec::new();
    for (name, args) in [
        ("logs/sample.csv", vec![]),
        ("logs/sample.json", vec![]),
        ("logs/sample.log", vec!["--prefix", PREFIX]),
    ] {
        let mut full = vec!["--output", "json"];
        full.extend(args);
        let out = analyze(&full, &[fixtures().join(name)])?;
        assert!(out.status.success(), "{name}: {}", stderr(&out));
        let value: serde_json::Value = serde_json::from_str(&stdout(&out))?;
        overviews.push(value["overview"]["events"].clone());
    }
    assert_eq!(overviews[0], overviews[1]);
    assert_eq!(overviews[0], overviews[2]);
    Ok(())
}

#[test]
fn stderr_prefix_is_inferred_and_shown() -> io::Result<()> {
    let out = analyze(&[], &[fixtures().join("logs/sample.log")])?;
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("prefix %m [%p] %q%u@%d/%a %Q  (inferred"),
        "{}",
        stdout(&out)
    );
    Ok(())
}

#[test]
fn an_unknown_prefix_exits_one_with_the_inference_report() -> io::Result<()> {
    let path = scratch(
        "odd.log",
        b"<2026-10-14T10:20:09> pid=42 LOG:  hello\n<2026-10-14T10:20:10> pid=42 LOG:  again\n",
    )?;
    let out = analyze(&[], std::slice::from_ref(&path))?;
    fs::remove_file(&path)?;
    assert_eq!(out.status.code(), Some(1));
    let message = stderr(&out);
    assert!(
        message.contains("could not infer the log_line_prefix"),
        "{message}"
    );
    assert!(
        message.contains("<2026-10-14T10:20:09> pid=42"),
        "{message}"
    );
    assert!(message.contains("--prefix"), "{message}");
    Ok(())
}

#[test]
fn threads_and_bucket_flags_are_accepted() -> io::Result<()> {
    let out = analyze(
        &[
            "--threads",
            "2",
            "--bucket",
            "30s",
            "--top",
            "1",
            "--by",
            "calls",
            "--max-fingerprints",
            "10",
        ],
        &[fixtures().join("logs/sample.json")],
    )?;
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("timeline (bucket 30s)"),
        "{}",
        stdout(&out)
    );
    Ok(())
}

#[test]
fn bad_arguments_are_usage_errors() -> io::Result<()> {
    for args in [
        vec!["--bucket", "soon"],
        vec!["--bucket", "0s"],
        vec!["--by", "bogus"],
        vec!["--threads", "0"],
    ] {
        let out = analyze(&args, &[fixtures().join("logs/sample.json")])?;
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", stderr(&out));
    }
    Ok(())
}

#[test]
fn several_files_are_merged() -> io::Result<()> {
    let one = fixtures().join("logs/sample.json");
    let out = analyze(&["--output", "json"], &[one.clone(), one])?;
    assert!(out.status.success(), "{}", stderr(&out));
    let value: serde_json::Value = serde_json::from_str(&stdout(&out))?;
    assert_eq!(value["overview"]["records"], 16);
    assert_eq!(value["files"].as_array().map(Vec::len), Some(2));
    Ok(())
}

#[test]
fn missing_file_and_binary_input_exit_one() -> io::Result<()> {
    let out = analyze(&[], &[PathBuf::from("/nonexistent/x.log")])?;
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("/nonexistent/x.log"),
        "{}",
        stderr(&out)
    );
    let bin = scratch("bin.bin", &[0, 1, 2, 0])?;
    let out = analyze(&[], std::slice::from_ref(&bin))?;
    fs::remove_file(&bin)?;
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("binary"), "{}", stderr(&out));
    Ok(())
}

#[test]
fn an_empty_file_is_a_report_with_warnings_not_an_error() -> io::Result<()> {
    let empty = scratch("empty.json", b"")?;
    let out = analyze(&[], std::slice::from_ref(&empty))?;
    fs::remove_file(&empty)?;
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("records 0"), "{}", stdout(&out));
    Ok(())
}

#[test]
fn closing_stdout_early_is_not_an_error() -> io::Result<()> {
    use std::process::Stdio;
    let mut child = Command::new(env!("CARGO_BIN_EXE_pgloupe"))
        .args(["logs", "analyze"])
        .arg(fixtures().join("logs/sample.json"))
        .stdout(Stdio::null())
        .spawn()?;
    assert!(child.wait()?.success());
    Ok(())
}
fn events_report() -> io::Result<serde_json::Value> {
    let out = analyze(
        &["--output", "json"],
        &[fixtures().join("analyze/events.json")],
    )?;
    assert!(out.status.success(), "{}", stderr(&out));
    Ok(serde_json::from_str(&stdout(&out))?)
}

#[test]
fn every_event_kind_is_counted() -> io::Result<()> {
    let report = events_report()?;
    let events = &report["overview"]["events"];
    for (kind, count) in [
        ("statement", 4),
        ("execute", 1),
        ("parse", 1),
        ("bind", 1),
        ("lock_wait", 1),
        ("lock_acquired", 1),
        ("deadlock", 1),
        ("temp_file", 2),
        ("error", 2),
        ("checkpoint_start", 1),
        ("checkpoint_done", 1),
        ("autovacuum", 2),
        ("connection_received", 1),
        ("connection_authenticated", 1),
        ("connection_authorized", 1),
        ("disconnection", 1),
        ("other", 1),
    ] {
        assert_eq!(events[kind], count, "{kind}: {events}");
    }
    assert_eq!(report["overview"]["records"], 23);
    Ok(())
}

#[test]
fn statements_are_grouped_and_timed() -> io::Result<()> {
    let report = events_report()?;
    let rows = report["statements"].as_array().unwrap();
    let orders = rows
        .iter()
        .find(|r| r["text"] == "SELECT * FROM orders WHERE id = $1")
        .expect("orders statement");
    assert_eq!(orders["calls"], 2);
    assert_eq!(orders["total_ms"], 40.0);
    assert_eq!(orders["mean_ms"], 20.0);
    assert_eq!(orders["query_id"], "111");
    let count = rows
        .iter()
        .find(|r| r["text"].as_str().unwrap().starts_with("SELECT count(*)"))
        .unwrap();
    assert_eq!(
        rows[0]["fingerprint"], count["fingerprint"],
        "the 500 ms join ranks first by total"
    );
    let bad = rows.iter().find(|r| r["text"] == "SELEC 1").unwrap();
    assert_eq!(bad["parsed"], false);
    assert_eq!(report["diagnostics"]["unparsed_statements"], 1);
    Ok(())
}

#[test]
fn errors_locks_and_temp_files_are_attributed() -> io::Result<()> {
    let report = events_report()?;
    let rows = report["statements"].as_array().unwrap();
    let insert = rows
        .iter()
        .find(|r| r["text"] == "INSERT INTO orders (id) VALUES ($1)")
        .unwrap();
    assert_eq!(insert["errors"], 2);
    let update = rows
        .iter()
        .find(|r| r["text"] == "UPDATE orders SET paid = $1 WHERE id = $2")
        .unwrap();
    assert_eq!(update["lock_waits"], 1);
    assert_eq!(update["lock_wait_ms"], 1200.3);
    let big = rows
        .iter()
        .find(|r| r["text"] == "SELECT * FROM big ORDER BY x")
        .unwrap();
    assert_eq!(
        (big["temp_files"].clone(), big["temp_bytes"].clone()),
        (2.into(), 12288.into())
    );
    assert_eq!(report["locks"]["deadlocks"], 1);
    assert_eq!(report["locks"]["max_wait_ms"], 1200.3);
    assert_eq!(report["temp_files"]["bytes"], 12288);
    let errors = report["errors"].as_array().unwrap();
    assert_eq!(errors[0]["sqlstate"], "23505");
    assert_eq!(errors[0]["count"], 2);
    assert_eq!(errors.len(), 2, "unique violations and the deadlock");
    Ok(())
}

#[test]
fn maintenance_and_connections_are_reported() -> io::Result<()> {
    let report = events_report()?;
    assert_eq!(report["checkpoints"][0]["kind"], "checkpoint");
    assert_eq!(report["checkpoints"][0]["buffers"], 100);
    assert_eq!(report["checkpoints"][0]["reasons"]["time"], 1);
    assert_eq!(report["autovacuum"][0]["table"], "shop.public.orders");
    assert_eq!(report["autovacuum"][0]["vacuums"], 1);
    assert_eq!(report["autovacuum"][0]["analyzes"], 1);
    assert_eq!(report["connections"]["authorized"], 1);
    assert_eq!(report["connections"]["session_ms"]["count"], 1);
    Ok(())
}

#[test]
fn text_report_matches_the_golden_file() -> io::Result<()> {
    // Run from the fixtures directory so the file path in the report is the same everywhere.
    let out = Command::new(env!("CARGO_BIN_EXE_pgloupe"))
        .current_dir(fixtures())
        .args(["logs", "analyze", "--bucket", "1m", "analyze/events.json"])
        .output()?;
    assert!(out.status.success(), "{}", stderr(&out));
    // The elapsed line varies per run; everything else is deterministic.
    let kept: Vec<String> = stdout(&out)
        .lines()
        .filter(|l| !l.starts_with("elapsed:"))
        .map(|l| format!("{l}\n"))
        .collect();
    let golden = fs::read_to_string(fixtures().join("analyze/events.golden.txt"))?;
    assert_eq!(kept.concat(), golden);
    Ok(())
}

#[test]
fn zero_records_from_a_non_empty_file_exit_one_with_a_report() -> io::Result<()> {
    let out = analyze(
        &["--prefix", "%t [%p]: "],
        &[fixtures().join("logs/sample.log")],
    )?;
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("records 0"), "{text}");
    assert!(text.contains("nothing could be decoded"), "{text}");
    Ok(())
}

fn slow_log() -> String {
    let line = |second: u32, severity: &str, message: &str, extra: &str| {
        format!(
            r#"{{"timestamp":"2026-10-14 10:00:{second:02}.000 UTC","pid":1,"session_id":"a.1","line_num":{second},"error_severity":"{severity}","message":"{message}"{extra},"backend_type":"client backend"}}"#
        ) + "\n"
    };
    let mut text = String::new();
    text.push_str(&line(
        1,
        "LOG",
        "duration: 5 ms  execute <unnamed>: SELECT * FROM t WHERE id = $1",
        r#","detail":"parameters: $1 = '1'""#,
    ));
    text.push_str(&line(
        2,
        "LOG",
        "duration: 90 ms  execute <unnamed>: SELECT * FROM t WHERE id = $1",
        r#","detail":"parameters: $1 = '2'""#,
    ));
    text.push_str(&line(
        3,
        "ERROR",
        "canceling statement due to statement timeout",
        r#","state_code":"57014","statement":"SELECT pg_sleep(60)""#,
    ));
    text
}

#[test]
fn slowest_executions_and_cancellations_are_listed_and_parameters_are_opt_in() -> io::Result<()> {
    let log = scratch("slow.json", slow_log().as_bytes())?;
    let plain = analyze(&[], std::slice::from_ref(&log))?;
    assert!(plain.status.success(), "{}", stderr(&plain));
    let text = stdout(&plain);
    assert!(text.contains("1 cancelled: statement_timeout=1"), "{text}");
    assert!(text.contains("90.00 ms"), "{text}");
    assert!(!text.contains("$1 = '2'"), "{text}");
    assert!(text.contains("no bind parameters"), "{text}");

    let with = analyze(&["--params", "--slowest", "1"], std::slice::from_ref(&log))?;
    let text = stdout(&with);
    assert!(text.contains("parameters: $1 = '2'"), "{text}");
    assert!(
        !text.contains("$1 = '1'"),
        "only the slowest is kept\n{text}"
    );
    Ok(())
}

fn two_databases() -> String {
    let line = |second: u32, db: &str, user: &str, sql: &str| {
        format!(
            r#"{{"timestamp":"2026-10-14 10:00:{second:02}.000 UTC","user":"{user}","dbname":"{db}","pid":1,"session_id":"a.1","line_num":{second},"error_severity":"LOG","message":"duration: 3 ms  statement: {sql}","application_name":"app","backend_type":"client backend"}}"#
        ) + "\n"
    };
    line(1, "shop", "app", "SELECT * FROM film")
        + &line(2, "shop", "monitor", "SELECT 1")
        + &line(3, "crm", "app", "SELECT * FROM actor")
}

fn statement_count(output: &Output) -> Result<u64, serde_json::Error> {
    let value: serde_json::Value = serde_json::from_str(&stdout(output))?;
    Ok(value["statements"].as_array().map_or(0, |rows| {
        rows.iter().filter_map(|r| r["calls"].as_u64()).sum()
    }))
}

#[test]
fn filters_choose_the_records_and_say_how_many_they_dropped() -> io::Result<()> {
    let log = scratch("filters.json", two_databases().as_bytes())?;
    let files = std::slice::from_ref(&log);
    let run = |extra: &[&str]| -> io::Result<Output> {
        let mut args = vec!["--output", "json"];
        args.extend(extra);
        analyze(&args, files)
    };
    let count = |extra: &[&str]| -> io::Result<u64> {
        let out = run(extra)?;
        assert!(out.status.success(), "{extra:?}: {}", stderr(&out));
        statement_count(&out).map_err(io::Error::other)
    };
    assert_eq!(count(&[])?, 3);
    assert_eq!(count(&["--database", "shop"])?, 2);
    assert_eq!(count(&["--database", "shop,crm"])?, 3);
    assert_eq!(count(&["--exclude-user", "monitor"])?, 2);
    assert_eq!(count(&["--database", "shop", "--user", "app"])?, 1);
    assert_eq!(count(&["--include-query", "(?i)from film"])?, 1);
    assert_eq!(count(&["--exclude-query", "film|actor"])?, 1);
    assert_eq!(
        count(&["--database", "shop", "--exclude-database", "shop"])?,
        0
    );

    let out = run(&["--database", "shop"])?;
    let value: serde_json::Value = serde_json::from_str(&stdout(&out)).map_err(io::Error::other)?;
    assert_eq!(value["diagnostics"]["filtered"], 1);
    let text = analyze(&["--database", "nope"], files)?;
    assert!(text.status.success(), "{}", stderr(&text));
    assert!(
        stdout(&text).contains("filtered out 3 records"),
        "{}",
        stdout(&text)
    );
    Ok(())
}

#[test]
fn bad_filters_are_usage_errors() -> io::Result<()> {
    let log = scratch("badfilter.json", two_databases().as_bytes())?;
    let out = analyze(&["--include-query", "("], std::slice::from_ref(&log))?;
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let store = Command::new(env!("CARGO_BIN_EXE_pgloupe"))
        .args(["report", "--store", "x.db", "--database", "shop"])
        .output()?;
    assert_eq!(store.status.code(), Some(2));
    assert!(
        stderr(&store).contains("filters apply while reading"),
        "{}",
        stderr(&store)
    );
    Ok(())
}

#[test]
fn literals_stay_out_of_the_slowest_list_unless_parameters_are_asked_for() -> io::Result<()> {
    let line = r#"{"timestamp":"2026-10-14 10:00:01.000 UTC","pid":1,"session_id":"a.1","line_num":1,"error_severity":"LOG","message":"duration: 50 ms  statement: SELECT * FROM customer WHERE email = 'alice@example.com'","backend_type":"client backend"}"#;
    let log = scratch("literals.json", format!("{line}\n").as_bytes())?;
    let files = std::slice::from_ref(&log);
    let plain = analyze(&[], files)?;
    assert!(
        !stdout(&plain).contains("alice@example.com"),
        "{}",
        stdout(&plain)
    );
    let with = analyze(&["--params"], files)?;
    assert!(
        stdout(&with).contains("alice@example.com"),
        "{}",
        stdout(&with)
    );
    Ok(())
}

#[test]
fn phases_cursors_prepared_statements_and_bare_durations_land_on_their_statement() -> io::Result<()>
{
    let out = analyze(
        &["--prefix", "%m [%p] %q%u@%d ", "--output", "json"],
        &[fixtures().join("logs/phases.log")],
    )?;
    assert!(out.status.success(), "{}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out))?;
    let rows = report["statements"].as_array().expect("statements");
    let find = |needle: &str| {
        rows.iter()
            .find(|r| r["text"].as_str().is_some_and(|t| t.contains(needle)))
            .unwrap_or_else(|| panic!("no statement with {needle}"))
    };
    // 900 ms of bind and 5 ms of parse on a statement that executed in 1 ms.
    let table = find("FROM t WHERE a");
    assert_eq!(table["phases"]["bind"]["total_ms"], 900.0);
    assert_eq!(table["phases"]["parse"]["total_ms"], 5.0);
    assert_eq!(table["total_ms"], 1.0);
    assert_eq!(table["all_ms"], 906.0);
    // The cursor's query owns its DECLARE and both FETCHes; neither is a row of its own.
    let big = find("FROM big");
    assert_eq!(big["phases"]["fetch"]["count"], 2);
    assert_eq!(big["phases"]["fetch"]["total_ms"], 500.0);
    assert!(
        rows.iter()
            .all(|r| !r["text"].as_str().unwrap_or("").contains("FETCH"))
    );
    // SQL-level EXECUTE counts as the prepared query; the bare duration times its statement.
    assert_eq!(find("FROM film").get("calls"), Some(&serde_json::json!(2)));
    let counted = find("count(*)");
    assert_eq!(
        (counted["calls"].as_u64(), counted["total_ms"].as_f64()),
        (Some(1), Some(50.0))
    );
    Ok(())
}

#[test]
fn time_of_day_weekday_pid_and_name_filters_narrow_the_analysis() -> io::Result<()> {
    let log = fixtures().join("logs/hours.log");
    let events = |extra: &[&str]| -> io::Result<u64> {
        let mut args = vec!["--prefix", "%m [%p] %q%u@%d/%a ", "--output", "json"];
        args.extend(extra);
        let out = analyze(&args, std::slice::from_ref(&log))?;
        assert!(out.status.success(), "{}", stderr(&out));
        let report: serde_json::Value = serde_json::from_str(&stdout(&out))?;
        Ok(report["overview"]["events"]["statement"]
            .as_u64()
            .unwrap_or(0))
    };
    assert_eq!(events(&[])?, 15);
    assert_eq!(events(&["--weekdays", "mon-fri"])?, 10);
    assert_eq!(events(&["--between", "09:00-18:00"])?, 6);
    assert_eq!(events(&["--exclude-hours", "3-6"])?, 9);
    assert_eq!(events(&["--pid", "106"])?, 3);
    assert_eq!(events(&["--application", "pgAdmin4*"])?, 3);
    assert_eq!(events(&["--exclude-application", "re:^pgAdmin"])?, 12);
    assert_eq!(events(&["--exclude-message", "SELECT 20 "])?, 12);
    // 12:00 UTC is 09:00 in Sao Paulo: the zone moves the window.
    assert_eq!(
        events(&["--between", "11:00-12:00", "--tz", "America/Sao_Paulo"])?,
        3
    );
    let bad = analyze(&["--between", "9-18"], &[log])?;
    assert!(!bad.status.success());
    Ok(())
}

#[test]
fn lock_waits_name_the_statement_that_held_the_lock() -> io::Result<()> {
    let out = analyze(
        &["--prefix", "%m [%p] %q%u@%d/%a ", "--output", "json"],
        &[fixtures().join("logs/blockers.log")],
    )?;
    assert!(out.status.success(), "{}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out))?;
    let blockers = report["locks"]["blockers"].as_array().expect("blockers");
    let pair = |holder: &str, waiter: &str| {
        blockers.iter().find(|b| {
            b["blocker_text"].as_str().unwrap_or("").contains(holder)
                && b["waiter_text"].as_str().unwrap_or("").contains(waiter)
        })
    };
    // Idle in its transaction: the holder is the last statement it ran, not the COMMIT.
    let idle = pair("balance - $1", "balance + $1").expect("idle holder");
    assert_eq!(idle["count"], 1);
    assert_eq!(idle["max_wait_ms"], 200.1);
    // One long statement: the holder is the statement that was running, not the BEGIN before.
    assert!(pair("WHERE batch", "ALTER TABLE").is_some(), "{blockers:?}");
    // A holder the log never shows is reported as such.
    let unknown = blockers
        .iter()
        .find(|b| b["blocker"].is_null())
        .expect("unknown holder");
    assert!(
        unknown["waiter_text"]
            .as_str()
            .unwrap_or("")
            .contains("DELETE")
    );
    assert_eq!(blockers.len(), 3);
    let text = stdout(&analyze(
        &["--prefix", "%m [%p] %q%u@%d/%a "],
        &[fixtures().join("logs/blockers.log")],
    )?);
    assert!(text.contains("who blocks whom"), "{text}");
    Ok(())
}
