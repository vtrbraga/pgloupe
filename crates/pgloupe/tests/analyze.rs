//! End-to-end tests of `pgloupe logs analyze`.

use std::{
    fmt::Write as _,
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
    assert_eq!(value["schema_version"], 2);
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

#[test]
fn files_are_correlated_on_their_own_and_filters_take_their_durations_with_them() -> io::Result<()>
{
    let a = scratch(
        "corr-a.log",
        b"2026-10-14 10:00:00.000 UTC [12] u@db/app LOG:  statement: SELECT pg_sleep(1)\n",
    )?;
    let b = scratch(
        "corr-b.log",
        b"2026-10-14 10:00:01.000 UTC [12] u@db/app LOG:  duration: 50.000 ms\n",
    )?;
    let total = |files: &[PathBuf], extra: &[&str]| -> io::Result<f64> {
        let mut args = vec!["--prefix", "%m [%p] %q%u@%d/%a ", "--output", "json"];
        args.extend(extra);
        let out = analyze(&args, files)?;
        assert!(out.status.success(), "{}", stderr(&out));
        let report: serde_json::Value = serde_json::from_str(&stdout(&out))?;
        Ok(report["statements"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|s| s["total_ms"].as_f64().unwrap_or(0.0))
            .sum())
    };
    // The statement and the bare duration are in different files: they are not linked either way.
    assert_eq!(total(&[a.clone(), b.clone()], &[])?, 0.0);
    assert_eq!(total(&[b.clone(), a.clone()], &[])?, 0.0);
    // In one file they are, and a filter on the statement removes both.
    let both = scratch(
        "corr-both.log",
        b"2026-10-14 10:00:00.000 UTC [12] u@db/app LOG:  statement: SELECT count(*) FROM rental\n\
          2026-10-14 10:00:01.000 UTC [12] u@db/app LOG:  duration: 50.000 ms\n",
    )?;
    assert_eq!(total(std::slice::from_ref(&both), &[])?, 50.0);
    assert_eq!(
        total(
            std::slice::from_ref(&both),
            &["--exclude-message", "rental"]
        )?,
        0.0
    );
    Ok(())
}

#[test]
fn repeated_application_messages_are_listed_by_shape_and_truncation_is_reported() -> io::Result<()>
{
    let log = fixtures().join("logs/chatter.log");
    let prefix = ["--prefix", "%t:%r:%u@%d:[%p]:", "--output", "json"];
    let out = analyze(&prefix, std::slice::from_ref(&log))?;
    assert!(out.status.success(), "{}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out))?;
    let messages = report["messages"].as_array().expect("messages");
    assert_eq!(
        messages[0]["shape"],
        "sync step ? finished for batch ? in ? s"
    );
    assert_eq!(messages[0]["count"], 60);
    assert!(
        messages
            .iter()
            .any(|m| m["shape"] == "cache warmed: \"?\"" && m["count"] == 3)
    );
    // The marker does not break the entry: its context and statement are kept, nothing skipped.
    assert_eq!(report["diagnostics"]["skipped"], 0);
    let warnings = report["diagnostics"]["warnings"].to_string();
    assert!(warnings.contains("cut short"), "{warnings}");
    // Under --anonymize the free text is gone, the counts stay.
    let mut hidden = prefix.to_vec();
    hidden.push("--anonymize");
    let out = analyze(&hidden, &[log])?;
    let text = stdout(&out);
    assert!(
        !text.contains("sync step") && !text.contains("cache warmed"),
        "{text}"
    );
    let report: serde_json::Value = serde_json::from_str(&text)?;
    assert_eq!(report["messages"][0]["shape"], "<redacted message>");
    assert_eq!(report["messages"][0]["count"], 64);
    Ok(())
}

#[test]
fn a_pgbouncer_log_is_detected_and_summarized() -> io::Result<()> {
    let log = fixtures().join("logs/pgbouncer.log");
    let out = analyze(&["--output", "json"], std::slice::from_ref(&log))?;
    assert!(out.status.success(), "{}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out))?;
    assert_eq!(report["files"][0]["format"], "pgbouncer");
    let pooler = &report["pooler"];
    assert_eq!(pooler["logins"], 30);
    assert_eq!(pooler["points"].as_array().map(Vec::len), Some(30));
    // The busiest minute is the last one: 40 + 29 transactions per second.
    assert_eq!(pooler["peak_xacts_s"], 69.0);
    let reasons = pooler["close_reasons"].as_array().expect("reasons");
    assert_eq!(reasons[0]["reason"], "client close request");
    assert_eq!(reasons[0]["count"], 30);
    assert!(reasons.iter().any(|r| r["reason"] == "query_wait_timeout"));
    // Nothing about statements or server settings is asked for from a pooler's log.
    let warnings = report["diagnostics"]["warnings"].to_string();
    assert!(!warnings.contains("durations"), "{warnings}");
    let text = stdout(&analyze(&[], &[log])?);
    assert!(text.contains("pgbouncer (experimental)"), "{text}");
    Ok(())
}

#[test]
fn a_log_with_the_second_rds_prefix_is_read_without_a_prefix_argument() -> io::Result<()> {
    let out = analyze(
        &["--output", "json"],
        &[fixtures().join("logs/rds_full_prefix.log")],
    )?;
    assert!(out.status.success(), "{}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out))?;
    let file = &report["files"][0];
    assert_eq!(file["prefix"], "%m:%r:%u@%d:[%p]:%l:%e:%s:%v:%x:%c:%q%a:");
    assert_eq!(file["prefix_source"], "inferred");
    assert_eq!(
        (file["records"].clone(), file["skipped"].clone()),
        (8.into(), 0.into())
    );
    assert_eq!(report["errors"][0]["sqlstate"], "42P01");
    assert_eq!(report["checkpoints"].as_array().map(Vec::len), Some(1));
    assert_eq!(report["overview"]["events"]["statement"], 1);
    assert_eq!(report["overview"]["events"]["execute"], 1);
    Ok(())
}

#[test]
fn a_prefix_without_q_is_diagnosed_as_such() -> io::Result<()> {
    let out = analyze(
        &[
            "--prefix",
            "%m:%r:%u@%d:[%p]:%l:%e:%s:%v:%x:%c:%a:",
            "--output",
            "json",
        ],
        &[fixtures().join("logs/rds_full_prefix.log")],
    )?;
    assert!(out.status.success(), "{}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out))?;
    let warnings: Vec<&str> = report["diagnostics"]["warnings"]
        .as_array()
        .map(|all| all.iter().filter_map(|w| w.as_str()).collect())
        .unwrap_or_default();
    assert!(
        warnings.iter().any(|w| w.contains("put %q")),
        "{warnings:?}"
    );
    assert!(
        !warnings.iter().any(|w| w.contains("lc_messages")),
        "{warnings:?}"
    );
    let not_seen = warnings.iter().find(|w| w.starts_with("not seen:"));
    assert!(
        not_seen.is_some_and(|w| !w.contains("checkpoints")),
        "{warnings:?}"
    );
    Ok(())
}

#[test]
fn a_log_in_portuguese_is_read_as_if_it_were_english() -> io::Result<()> {
    let log = fixtures().join("logs/pt_br.log");
    let prefix = ["--prefix", "%m [%p] %q%u@%d/%a ", "--output", "json"];
    let events = |extra: &[&str]| -> io::Result<serde_json::Value> {
        let mut args = prefix.to_vec();
        args.extend(extra);
        let out = analyze(&args, std::slice::from_ref(&log))?;
        assert!(out.status.success(), "{}", stderr(&out));
        Ok(serde_json::from_str(&stdout(&out))?)
    };
    for extra in [&[][..], &["--lc-messages", "pt_BR"][..]] {
        let report = events(extra)?;
        let overview = &report["overview"]["events"];
        // Three plain ones, one with `": "` in its SQL, one logged without a duration.
        assert_eq!(overview["statement"], 5, "{extra:?}");
        assert_eq!(overview["execute"], 1, "{extra:?}");
        assert_eq!(
            report["locks"]["blockers"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(overview["lock_wait"], 1);
        assert_eq!(overview["lock_acquired"], 1);
        assert_eq!(overview["temp_file"], 1);
        assert_eq!(overview["disconnection"], 1);
        assert_eq!(report["overview"]["levels"]["ERROR"], 1);
        assert_eq!(report["overview"]["levels"]["WARNING"], 1);
        assert_eq!(report["files"][0]["locale"], "pt_BR");
        assert_eq!(report["errors"][0]["sqlstate"], serde_json::Value::Null);
        assert_eq!(report["locks"]["waits"], 1);
    }
    // Read as English, nothing is recognized.
    let report = events(&["--lc-messages", "en"])?;
    assert_eq!(
        report["overview"]["events"]["statement"],
        serde_json::Value::Null
    );
    // An unknown language says what is available.
    let out = analyze(
        &["--prefix", "%m [%p] %q%u@%d/%a ", "--lc-messages", "xx"],
        std::slice::from_ref(&log),
    )?;
    assert!(!out.status.success());
    assert!(stderr(&out).contains("pt_BR"), "{}", stderr(&out));
    Ok(())
}

const BLOCKED: &str = "\
2026-10-14 10:00:00.000 UTC [20] holder@db/app LOG:  duration: 4.000 ms  statement: UPDATE accounts SET balance = 0 WHERE id = 1
2026-10-14 10:00:01.000 UTC [21] waiter@db/app LOG:  process 21 still waiting for ShareLock on transaction 500 after 200.100 ms
2026-10-14 10:00:01.000 UTC [21] waiter@db/app DETAIL:  Process holding the lock: 20. Wait queue: 21.
2026-10-14 10:00:01.000 UTC [21] waiter@db/app STATEMENT:  UPDATE accounts SET balance = 1 WHERE id = 1
";

fn blockers_of(args: &[&str], log: &[u8], name: &str) -> io::Result<usize> {
    let file = scratch(name, log)?;
    let mut all = vec!["--prefix", "%m [%p] %q%u@%d/%a ", "--output", "json"];
    all.extend_from_slice(args);
    let out = analyze(&all, &[file])?;
    assert!(out.status.success(), "{}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out))?;
    Ok(report["locks"]["blockers"].as_array().map_or(0, Vec::len))
}

#[test]
fn a_lock_wait_is_blamed_even_when_the_holders_records_are_filtered_out() -> io::Result<()> {
    assert_eq!(blockers_of(&[], BLOCKED.as_bytes(), "blk-all.log")?, 1);
    // A wait whose waiter the filter drops is blamed on no one, and its SQL stays out.
    assert_eq!(
        blockers_of(&["--user", "nobody"], BLOCKED.as_bytes(), "blk-none.log")?,
        0
    );
    assert_eq!(
        blockers_of(
            &["--exclude-hours", "10"],
            BLOCKED.as_bytes(),
            "blk-hours.log"
        )?,
        0
    );
    assert_eq!(
        blockers_of(
            &["--user", "waiter"],
            format!("{BLOCKED}2026-10-14 10:00:05.000 UTC [20] holder@db/app LOG:  duration: 0.5 ms  statement: COMMIT\n").as_bytes(),
            "blk-user.log"
        )?,
        1
    );
    Ok(())
}

#[test]
fn a_lock_wait_still_pending_at_a_batch_boundary_is_blamed() -> io::Result<()> {
    // The file ends with the wait: no later record exists to carry the pair.
    let mut log = String::new();
    for i in 0..4094 {
        writeln!(
            log,
            "2026-10-14 10:00:00.000 UTC [9{i}] u@db/app LOG:  duration: 1.000 ms  statement: SELECT {i}"
        )
        .unwrap();
    }
    log.push_str(&BLOCKED.replace(
        "duration: 4.000 ms  statement: UPDATE accounts SET balance = 0 WHERE id = 1",
        "connection received: host=1.2.3.4",
    ));
    assert_eq!(blockers_of(&[], log.as_bytes(), "blk-edge.log")?, 1);
    Ok(())
}

#[test]
fn pg_stat_database_adds_a_server_wide_section_and_findings() -> io::Result<()> {
    let stats = fixtures().join("stats");
    let out = Command::new(env!("CARGO_BIN_EXE_pgloupe"))
        .args(["report", "--before"])
        .arg(stats.join("basic_before.csv"))
        .arg("--after")
        .arg(stats.join("basic_after.csv"))
        .arg("--before-db")
        .arg(stats.join("db_before.csv"))
        .arg("--after-db")
        .arg(stats.join("db_after.csv"))
        .args(["--output", "json"])
        .output()?;
    assert!(out.status.success(), "{}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out))?;
    let db = &report["database"];
    assert_eq!(db["window_secs"], 600.0);
    assert_eq!(db["total"]["commits"], 96_000);
    assert_eq!(db["total"]["rollbacks"], 9_002);
    assert_eq!(db["databases"][0]["name"], "shop", "busiest first");
    let codes: Vec<&str> = report["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f["code"].as_str())
        .collect();
    for wanted in [
        "statements_cover_little",
        "many_rollbacks",
        "sessions_abandoned",
        "idle_in_transaction",
    ] {
        assert!(codes.contains(&wanted), "{wanted} in {codes:?}");
    }
    // Without the files nothing changes.
    let plain = Command::new(env!("CARGO_BIN_EXE_pgloupe"))
        .args(["report", "--before"])
        .arg(stats.join("basic_before.csv"))
        .arg("--after")
        .arg(stats.join("basic_after.csv"))
        .args(["--output", "json"])
        .output()?;
    let plain: serde_json::Value = serde_json::from_str(&stdout(&plain))?;
    assert!(plain.get("database").is_none());
    Ok(())
}

#[test]
fn statements_carry_cache_hit_io_share_and_blocks_per_row() -> io::Result<()> {
    let stats = fixtures().join("stats");
    let out = Command::new(env!("CARGO_BIN_EXE_pgloupe"))
        .args(["report", "--before"])
        .arg(stats.join("io_before.csv"))
        .arg("--after")
        .arg(stats.join("io_after.csv"))
        .args(["--output", "json"])
        .output()?;
    assert!(out.status.success(), "{}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out))?;
    let find = |needle: &str| {
        report["statements"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|s| s["text"].as_str().is_some_and(|t| t.contains(needle)))
            .expect(needle)["stats"]
            .clone()
    };
    // 200 calls and 4,000 ms; 20,000 hits and 10,000 reads; 2,000 ms reading, 0 writing, 150 ms
    // of temp files; 400 rows.
    let orders = find("orders");
    assert_eq!(orders["calls"], 200);
    assert!((orders["hit_pct"].as_f64().unwrap() - 66.666).abs() < 0.01);
    assert!(
        (orders["io_pct"].as_f64().unwrap() - 53.75).abs() < 0.01,
        "{orders}"
    );
    assert!((orders["rows_per_call"].as_f64().unwrap() - 2.0).abs() < 1e-9);
    assert!((orders["blocks_read_per_row"].as_f64().unwrap() - 25.0).abs() < 1e-9);
    // Twice the mean since before the window, and an uneven run time (60 ms spread on a 17 ms mean).
    assert!(
        orders["flags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == "slower"),
        "{orders}"
    );
    assert_eq!(orders["max_ms"], 900.0);
    assert!(
        (orders["variability"].as_f64().unwrap() - 3.6).abs() < 0.01,
        "{orders}"
    );
    // A delete returning nothing per call but reading thousands of blocks and writing WAL.
    let delete = find("DELETE");
    assert_eq!(delete["wal_per_call"], 200_000.0);
    let codes: Vec<&str> = report["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f["code"].as_str())
        .collect();
    assert!(codes.contains(&"io_heavy"), "{codes:?}");
    assert!(codes.contains(&"low_cache_hit"), "{codes:?}");
    let stats = &report["inputs"]["stats"];
    assert_eq!(stats["by_database"][0]["name"], "shop");
    assert_eq!(stats["by_database"][0]["calls"], 210);
    assert_eq!(stats["by_user"][0]["name"], "app");
    assert!((stats["by_user"][0]["share_pct"].as_f64().unwrap() - 100.0).abs() < 1e-9);
    let totals = &report["inputs"]["stats"]["totals"];
    assert_eq!(totals["calls"], 210);
    assert!(
        (totals["hit_pct"].as_f64().unwrap() - 60.17).abs() < 0.05,
        "{totals}"
    );
    assert!(totals["io_pct"].as_f64().unwrap() > 30.0, "{totals}");
    assert!(delete["io_pct"].as_f64().unwrap() > 20.0);
    Ok(())
}

#[test]
fn the_shares_of_the_kinds_add_up_when_statements_have_phases() -> io::Result<()> {
    let out = analyze(
        &["--prefix", "%m [%p] %q%u@%d ", "--output", "json"],
        &[fixtures().join("logs/phases.log")],
    )?;
    assert!(out.status.success(), "{}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out))?;
    let kinds: f64 = report["kinds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|k| k["share_pct"].as_f64())
        .sum();
    let statements: f64 = report["statements"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s["share_pct"].as_f64())
        .sum();
    assert!((kinds - 100.0).abs() < 0.5, "kinds add up to {kinds}");
    assert!(
        (statements - 100.0).abs() < 0.5,
        "statements add up to {statements}"
    );
    Ok(())
}
