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
