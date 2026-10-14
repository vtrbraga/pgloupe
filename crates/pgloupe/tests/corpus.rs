//! Runs `stats diff` on the synthetic corpus. Needs `cargo run -p synth -- generate`.

use std::{
    io,
    path::PathBuf,
    process::{Command, Output},
};

fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!(
            "../../data/synth/{}-42",
            std::env::var("PGLOUPE_PG").unwrap_or_else(|_| "17".to_owned())
        ))
        .join(name)
}

fn diff(before: &str, after: &str, before_info: &str, after_info: &str) -> io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_pgloupe"))
        .args(["stats", "diff"])
        .arg(corpus(before))
        .arg(corpus(after))
        .arg("--before-info")
        .arg(corpus(before_info))
        .arg("--after-info")
        .arg(corpus(after_info))
        .output()
}

fn squashed(out: &Output) -> Vec<String> {
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect()
}

#[test]
#[ignore = "needs data/synth/17-42 (cargo run -p synth -- generate)"]
fn baseline_window_is_clean_and_churn_window_is_a_proven_reset() -> io::Result<()> {
    let clean = diff(
        "pgss_0.csv",
        "pgss_1.csv",
        "pgss_info_0.csv",
        "pgss_info_1.csv",
    )?;
    assert!(
        clean.status.success(),
        "{}",
        String::from_utf8_lossy(&clean.stderr)
    );
    let lines = squashed(&clean);
    assert!(lines.iter().any(|l| l == "reset: none"), "{lines:#?}");
    assert!(lines.iter().any(|l| l == "evictions: none"), "{lines:#?}");

    let churn = diff(
        "pgss_1.csv",
        "pgss_2.csv",
        "pgss_info_1.csv",
        "pgss_info_2.csv",
    )?;
    assert!(
        churn.status.success(),
        "{}",
        String::from_utf8_lossy(&churn.stderr)
    );
    let lines = squashed(&churn);
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("reset: proven; window starts at stats_reset")),
        "{lines:#?}"
    );
    assert!(lines.iter().any(|l| l == "evictions: 5"), "{lines:#?}");
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("note: counters were reset"))
    );
    Ok(())
}

#[test]
#[ignore = "needs data/synth/17-42 (cargo run -p synth -- generate)"]
fn churn_window_without_info_files_says_the_reset_is_unknown() -> io::Result<()> {
    let out = Command::new(env!("CARGO_BIN_EXE_pgloupe"))
        .args(["stats", "diff"])
        .arg(corpus("pgss_1.csv"))
        .arg(corpus("pgss_2.csv"))
        .output()?;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let lines = squashed(&out);
    assert!(
        lines.iter().any(|l| l.starts_with("reset: unknown")),
        "{lines:#?}"
    );
    Ok(())
}
