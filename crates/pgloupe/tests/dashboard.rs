//! Runs the dashboard's pure JavaScript helpers under `node --test`.

use std::{
    io,
    path::PathBuf,
    process::{Command, Stdio},
};

#[test]
fn the_dashboard_helpers_pass_their_javascript_tests() -> io::Result<()> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/dashboard");
    match Command::new("node")
        .arg("--test")
        .arg(&dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
    {
        Ok(output) => {
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            eprintln!("skipped: `node` is not installed, so the dashboard helpers were not tested");
        }
        Err(error) => return Err(error),
    }
    Ok(())
}

/// Renders a generated report in headless Chrome and fails on any script error.
/// Set `PGLOUPE_CHROME` to a headless Chrome binary; without it the test is skipped.
#[test]
fn the_dashboard_renders_without_script_errors() -> io::Result<()> {
    let Some(chrome) = std::env::var_os("PGLOUPE_CHROME") else {
        eprintln!("skipped: PGLOUPE_CHROME is not set");
        return Ok(());
    };
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let dir = std::env::temp_dir().join(format!("pgloupe-dash-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let html = dir.join("report.html");
    // A "before" snapshot sharing no statement with "after": every statement is new, which hides the flag.
    let before = dir.join("before.csv");
    let header = std::fs::read_to_string(fixtures.join("stats/basic_before.csv"))?;
    let mut lines = header.lines();
    let first = lines.next().unwrap_or_default();
    let other = lines.next().unwrap_or_default().replace(",t,1,", ",t,999,");
    std::fs::write(&before, format!("{first}\n{other}\n"))?;
    let made = Command::new(env!("CARGO_BIN_EXE_pgloupe"))
        .args(["report", "--output", "html", "--out"])
        .arg(&html)
        .arg(fixtures.join("combined/logs.json"))
        .arg("--before")
        .arg(&before)
        .arg("--after")
        .arg(fixtures.join("stats/basic_after.csv"))
        .output()?;
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );
    let page = Command::new(chrome)
        .args([
            "--no-sandbox",
            "--disable-gpu",
            "--enable-logging=stderr",
            "--virtual-time-budget=3000",
            "--dump-dom",
        ])
        .arg(format!("file://{}", html.display()))
        .output()?;
    let log = String::from_utf8_lossy(&page.stderr);
    let dom = String::from_utf8_lossy(&page.stdout);
    assert!(!log.contains("Uncaught"), "script error:\n{log}");
    assert!(dom.contains("Statements"), "dashboard did not render");
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
