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
