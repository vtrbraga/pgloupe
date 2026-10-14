//! End-to-end tests of `pgloupe stats diff` on the fixtures in `fixtures/stats/`.

use std::{
    io,
    path::PathBuf,
    process::{Command, Output},
};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/stats")
        .join(name)
}

fn pgloupe(args: &[&str], files: &[&str]) -> io::Result<Output> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pgloupe"));
    command.args(args);
    for file in files {
        command.arg(fixture(file));
    }
    command.output()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Whitespace-collapsed lines, so tests do not depend on column widths.
fn squash(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect()
}

#[test]
fn text_report_for_a_clean_window() -> io::Result<()> {
    let out = pgloupe(&["stats", "diff"], &["basic_before.csv", "basic_after.csv"])?;
    assert!(out.status.success(), "{}", stderr(&out));
    let lines = squash(&stdout(&out));
    for expected in [
        "window: 2026-10-14 10:00:00 UTC -> 2026-10-14 10:10:00 UTC (10m 00s)",
        "reset: none",
        "evictions: unknown (no info files)",
        "entries: 4 in after: 1 new, 0 restarted, 1 unchanged; 1 gone; 0 skipped (no queryid)",
        "400.00 80.0 4 20.000→42.857 4 ok 2 UPDATE film SET rental_rate = $1",
        "70.00 14.0 2 -→35.000 2 new 5 DELETE FROM rental WHERE rental_id = $1",
        "30.00 6.0 60 0.500→0.500 60 ok 1 SELECT * FROM film WHERE film_id = $1",
    ] {
        assert!(
            lines.iter().any(|line| line == expected),
            "missing {expected:?} in {lines:#?}"
        );
    }
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("note: without info files or stats_since"))
    );
    Ok(())
}

#[test]
fn by_calls_changes_the_order() -> io::Result<()> {
    let out = pgloupe(
        &["stats", "diff", "--by", "calls", "--top", "1"],
        &["basic_before.csv", "basic_after.csv"],
    )?;
    assert!(out.status.success(), "{}", stderr(&out));
    let lines = squash(&stdout(&out));
    assert!(lines.iter().any(|l| l.starts_with("30.00 6.0 60 ")));
    assert!(!lines.iter().any(|l| l.starts_with("400.00 ")));
    Ok(())
}

#[test]
fn proven_reset_with_info_files_is_flagged_and_exits_zero() -> io::Result<()> {
    let out = Command::new(env!("CARGO_BIN_EXE_pgloupe"))
        .args(["stats", "diff"])
        .arg(fixture("reset_before.csv"))
        .arg(fixture("reset_after.csv"))
        .arg("--before-info")
        .arg(fixture("reset_before_info.csv"))
        .arg("--after-info")
        .arg(fixture("reset_after_info.csv"))
        .output()?;
    assert!(out.status.success(), "{}", stderr(&out));
    let lines = squash(&stdout(&out));
    for expected in [
        "reset: proven; window starts at stats_reset 2026-10-14 10:05:00 UTC",
        "evictions: 2",
        "entries: 2 in after: 1 new, 1 restarted, 0 unchanged; 1 gone; 0 skipped (no queryid)",
        "9.00 72.0 3 -→3.000 3 new,partial,evicted-suspect 3 SELECT count(*) FROM actor",
        "3.50 28.0 7 0.500→0.500 7 restarted,partial 1 SELECT * FROM film WHERE film_id = $1",
    ] {
        assert!(
            lines.iter().any(|line| line == expected),
            "missing {expected:?} in {lines:#?}"
        );
    }
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("note: counters were reset"))
    );
    assert!(
        !lines
            .iter()
            .any(|l| l.starts_with("note: without info files"))
    );
    Ok(())
}

#[test]
fn json_output_is_valid_and_versioned() -> io::Result<()> {
    let out = pgloupe(
        &["stats", "diff", "--format", "json"],
        &["basic_before.csv", "basic_after.csv"],
    )?;
    assert!(out.status.success(), "{}", stderr(&out));
    let value: serde_json::Value = serde_json::from_str(&stdout(&out))?;
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["verdict"]["reset"], "none");
    assert!(value["verdict"]["evictions"].is_null());
    assert_eq!(value["rows"][0]["queryid"], "2");
    assert_eq!(value["rows"][0]["delta"]["calls"], 4);
    assert_eq!(value["rows"][0]["status"], "ok");
    Ok(())
}

#[test]
fn different_major_versions_exit_one_with_the_reason() -> io::Result<()> {
    let out = pgloupe(&["stats", "diff"], &["basic_before.csv", "v16_after.csv"])?;
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out), "");
    assert!(
        stderr(&out).contains("different Postgres major versions"),
        "{}",
        stderr(&out)
    );
    Ok(())
}

#[test]
fn swapped_snapshots_exit_one() -> io::Result<()> {
    let out = pgloupe(&["stats", "diff"], &["basic_after.csv", "basic_before.csv"])?;
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("is not later than"),
        "{}",
        stderr(&out)
    );
    Ok(())
}

#[test]
fn legacy_columns_exit_one_with_a_hint() -> io::Result<()> {
    let out = pgloupe(&["stats", "diff"], &["legacy.csv", "legacy.csv"])?;
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("PG13"), "{}", stderr(&out));
    Ok(())
}

#[test]
fn missing_file_exit_one_names_the_path() -> io::Result<()> {
    let out = Command::new(env!("CARGO_BIN_EXE_pgloupe"))
        .args(["stats", "diff", "/nonexistent/a.csv", "/nonexistent/b.csv"])
        .output()?;
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("/nonexistent/a.csv"),
        "{}",
        stderr(&out)
    );
    Ok(())
}

#[test]
fn info_flags_must_be_given_together() -> io::Result<()> {
    let out = Command::new(env!("CARGO_BIN_EXE_pgloupe"))
        .args(["stats", "diff"])
        .arg(fixture("basic_before.csv"))
        .arg(fixture("basic_after.csv"))
        .arg("--before-info")
        .arg(fixture("reset_before_info.csv"))
        .output()?;
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("must be given together"),
        "{}",
        stderr(&out)
    );
    Ok(())
}

#[test]
fn usage_errors_exit_two() -> io::Result<()> {
    let out = Command::new(env!("CARGO_BIN_EXE_pgloupe"))
        .args(["stats", "diff", "--by", "bogus", "a", "b"])
        .output()?;
    assert_eq!(out.status.code(), Some(2));
    Ok(())
}
