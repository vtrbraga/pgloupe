//! `snap` against real servers. Needs Docker; run with `-- --ignored`.

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test code")]

mod support;

use std::{fs, path::PathBuf};

use pgloupe::{
    collect::{conn::ConnSpec, sql},
    stats::snapshot::Snapshot,
};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pgloupe-snap-{}-{name}", std::process::id()));
    drop(fs::remove_dir_all(&dir));
    dir
}

fn snap_on(major: u32) -> Result<(), String> {
    let pg = support::Pg::start(major, &[])?;
    pg.sql("CREATE TABLE t (a int); INSERT INTO t SELECT g FROM generate_series(1, 50) g; SELECT count(*) FROM t WHERE a > 10")?;
    let spec = ConnSpec::parse(&pg.url(), None, None).map_err(|e| e.to_string())?;
    let mut client = spec.connect().map_err(|e| e.to_string())?;
    let dir = scratch(&major.to_string());

    let first = sql::take(&mut client, &dir).map_err(|e| e.to_string())?;
    pg.sql("SELECT count(*) FROM t WHERE a > 20")?;
    std::thread::sleep(std::time::Duration::from_millis(1_100));
    let second = sql::take(&mut client, &dir).map_err(|e| e.to_string())?;

    // The longer copy timeout ended with the snapshot transaction.
    let timeout: String = client
        .query_one("SHOW statement_timeout", &[])
        .map_err(|e| e.to_string())?
        .get(0);
    assert_eq!(timeout, "15s");

    // Two snapshots a second apart are two files that read back as snapshots. Their order is not
    // asserted: the server clock this runs against can step back.
    assert_ne!(first.pgss, second.pgss);
    let before = Snapshot::from_path(&first.pgss).map_err(|e| e.to_string())?;
    let after = Snapshot::from_path(&second.pgss).map_err(|e| e.to_string())?;
    assert!(!after.entries.is_empty());
    if major >= 14 {
        assert!(first.info.as_ref().is_some_and(|p| p.exists()), "info file");
    }

    // The server's settings come with each snapshot, in the format a report reads.
    let settings = first.settings.as_ref().ok_or("no settings file")?;
    let read = pgloupe::combined::read_settings(settings)?;
    assert_eq!(
        read.get("server_version_num")
            .map(String::as_str)
            .map(str::len),
        Some(6)
    );
    assert!(read.contains_key("log_min_duration_statement"));
    assert!(read["shared_preload_libraries"].contains("pg_stat_statements"));

    // Read-only: writing fails, and the statistics were not reset.
    let write = client
        .batch_execute("CREATE TABLE nope (a int)")
        .err()
        .ok_or("a write succeeded in a read-only session")?;
    let message = write
        .as_db_error()
        .map(|e| e.message().to_owned())
        .unwrap_or_default();
    assert!(message.contains("read-only"), "{message}");
    assert!(after.entries.len() >= before.entries.len());

    // The password never reaches the files or the description.
    assert!(!spec.describe().contains(&pg.password));
    for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
        let text = fs::read_to_string(entry.map_err(|e| e.to_string())?.path())
            .map_err(|e| e.to_string())?;
        assert!(!text.contains(&pg.password));
    }
    Ok(())
}

#[test]
#[ignore = "needs Docker"]
fn snapshots_work_on_postgres_14_to_18() {
    for major in [14, 15, 16, 17, 18] {
        snap_on(major).unwrap_or_else(|e| panic!("postgres {major}: {e}"));
    }
}

#[test]
#[ignore = "needs Docker"]
fn the_columns_match_the_synthetic_corpus() -> Result<(), String> {
    let corpus =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/synth/17-42/pgss_0.csv");
    if !corpus.exists() {
        return Ok(());
    }
    let expected = fs::read_to_string(corpus).map_err(|e| e.to_string())?;
    let expected_header = expected.lines().next().unwrap_or_default().to_owned();
    let pg = support::Pg::start(17, &[])?;
    let spec = ConnSpec::parse(&pg.url(), None, None).map_err(|e| e.to_string())?;
    let mut client = spec.connect().map_err(|e| e.to_string())?;
    let dir = scratch("header");
    let snap = sql::take(&mut client, &dir).map_err(|e| e.to_string())?;
    let got = fs::read_to_string(snap.pgss).map_err(|e| e.to_string())?;
    assert_eq!(got.lines().next().unwrap_or_default(), expected_header);
    Ok(())
}

#[test]
#[ignore = "needs Docker"]
fn a_server_without_the_extension_says_how_to_enable_it() -> Result<(), String> {
    let pg = support::Pg::start(16, &[])?;
    pg.sql("DROP EXTENSION pg_stat_statements")?;
    let spec = ConnSpec::parse(&pg.url(), None, None).map_err(|e| e.to_string())?;
    let mut client = spec.connect().map_err(|e| e.to_string())?;
    let err = sql::take(&mut client, &scratch("missing"))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("shared_preload_libraries") && err.contains("CREATE EXTENSION"),
        "{err}"
    );
    Ok(())
}

fn pgloupe(args: &[&str], envs: &[(&str, &str)]) -> std::io::Result<std::process::Output> {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_pgloupe"));
    command.args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output()
}

#[test]
#[ignore = "needs Docker"]
fn the_commands_work_end_to_end_and_never_print_the_password() -> Result<(), String> {
    let pg = support::Pg::start(16, &["log_min_duration_statement=-1"])?;
    pg.sql("CREATE TABLE t (a int); SELECT count(*) FROM t")?;
    let dir = scratch("cli");
    let dsn = pg.url();
    let snap = |n: u32| -> Result<std::process::Output, String> {
        let _ = n;
        pgloupe(
            &["snap", "--out", dir.to_str().unwrap_or_default()],
            &[("PGLOUPE_DSN", &dsn)],
        )
        .map_err(|e| e.to_string())
    };
    let first = snap(1)?;
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    std::thread::sleep(std::time::Duration::from_millis(1_100));
    let second = snap(2)?;
    let text = String::from_utf8_lossy(&second.stdout).into_owned();
    assert!(text.contains("pgloupe stats diff"), "{text}");

    let doctor = pgloupe(&["doctor", "--vendor", "rds"], &[("PGLOUPE_DSN", &dsn)])
        .map_err(|e| e.to_string())?;
    let report = String::from_utf8_lossy(&doctor.stdout).into_owned();
    assert!(doctor.status.success());
    assert!(report.contains("no_statement_timings"), "{report}");
    assert!(report.contains("parameter group"), "{report}");

    for out in [&first, &second, &doctor] {
        let all = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!all.contains(&pg.password), "the password leaked: {all}");
    }
    // A wrong password fails cleanly and without echoing either password.
    let wrong = dsn.replace(&pg.password, "nope-nope");
    let bad = pgloupe(
        &["snap", "--out", dir.to_str().unwrap_or_default()],
        &[("PGLOUPE_DSN", &wrong)],
    )
    .map_err(|e| e.to_string())?;
    assert_eq!(bad.status.code(), Some(1));
    let all = String::from_utf8_lossy(&bad.stderr).into_owned();
    assert!(
        !all.contains("nope-nope") && !all.contains(&pg.password),
        "{all}"
    );
    Ok(())
}
