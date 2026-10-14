#![expect(clippy::unwrap_used, reason = "test helpers panic on setup failure")]

mod common;

use std::{
    fs,
    os::unix::fs::MetadataExt as _,
    time::{Duration, Instant},
};

use synth::{
    container::{PgContainer, RunSpec, docker, remove_stale},
    session::{self, Target},
    snapshot,
};

fn start(name: &str, label: &str) -> PgContainer {
    let dir = common::scratch_dir(name);
    let log = dir.join("log");
    let dataset = dir.join("dataset");
    fs::create_dir_all(&log).unwrap();
    fs::create_dir_all(&dataset).unwrap();
    let meta = fs::metadata(&log).unwrap();
    let container_name = common::unique(name);
    PgContainer::start(&RunSpec {
        name: &container_name,
        image: "postgres:17",
        label,
        uid: meta.uid(),
        gid: meta.gid(),
        log_dir: &log,
        dataset_dir: &dataset,
    })
    .unwrap()
}

#[test]
#[ignore = "needs docker"]
fn container_accepts_queries_and_is_removed_on_drop() {
    let label = format!("pgloupe.synth.test={}", common::unique("drop"));
    let pg = start("drop", &label);
    assert!(pg.port() > 0);
    let out = pg
        .exec_unlogged(&["psql", "-U", "postgres", "-Atc", "SELECT 1"])
        .unwrap();
    assert_eq!(out, "1");
    let name = pg.name().to_owned();
    drop(pg);
    let filter = format!("name={name}");
    let left = docker(&["ps", "-aq", "--filter", filter.as_str()]).unwrap();
    assert!(left.is_empty(), "container {name} still exists");
}

#[test]
#[ignore = "needs docker"]
fn remove_stale_removes_only_labelled_containers() {
    let label = format!("pgloupe.synth.test={}", common::unique("stale"));
    let pg = start("stale", &label);
    let name = pg.name().to_owned();
    std::mem::forget(pg);
    remove_stale(&label).unwrap();
    let filter = format!("name={name}");
    let left = docker(&["ps", "-aq", "--filter", filter.as_str()]).unwrap();
    assert_eq!(left, "");
}

#[test]
#[ignore = "needs docker"]
fn snapshot_writes_three_csv_files_with_headers() {
    let label = format!("pgloupe.synth.test={}", common::unique("snap"));
    let pg = start("snap", &label);
    let mut control = Target { port: pg.port() }.control("postgres").unwrap();
    control
        .batch_execute("CREATE EXTENSION pg_stat_statements")
        .unwrap();
    let dir = common::scratch_dir("snap-out");
    let at = snapshot::take(&mut control, &dir, 0).unwrap();
    assert!(at.ends_with("+00"), "server time should be UTC text: {at}");
    let pgss = fs::read_to_string(dir.join("pgss_0.csv")).unwrap();
    assert!(pgss.lines().next().unwrap().contains("queryid"));
    assert!(
        fs::read_to_string(dir.join("pgss_info_0.csv"))
            .unwrap()
            .contains("dealloc")
    );
    assert!(
        fs::read_to_string(dir.join("pg_stat_database_0.csv"))
            .unwrap()
            .contains("datname")
    );
}

#[test]
#[ignore = "needs docker"]
fn wait_for_lock_wait_times_out_for_idle_backend() {
    let label = format!("pgloupe.synth.test={}", common::unique("idle"));
    let pg = start("idle", &label);
    let target = Target { port: pg.port() };
    let mut idle = target
        .connect("postgres", "postgres", "synth:test")
        .unwrap();
    let pid = session::backend_pid(&mut idle).unwrap();
    let mut control = target.control("postgres").unwrap();
    let started = Instant::now();
    session::wait_for_lock_wait(&mut control, pid, Duration::from_millis(300)).unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
#[ignore = "needs docker"]
fn setup_sessions_are_not_tracked_by_pgss() {
    let label = format!("pgloupe.synth.test={}", common::unique("untracked"));
    let pg = start("untracked", &label);
    let mut control = Target { port: pg.port() }.control("postgres").unwrap();
    control
        .batch_execute("CREATE EXTENSION pg_stat_statements")
        .unwrap();
    control
        .batch_execute("SELECT 1 AS synth_marker_control")
        .unwrap();
    pg.exec_unlogged(&[
        "psql",
        "-U",
        "postgres",
        "-Atc",
        "SELECT 1 AS synth_marker_exec",
    ])
    .unwrap();
    let tracked = session::query_scalar(
        &mut control,
        "SELECT count(*) FROM pg_stat_statements \
         WHERE query LIKE '%synth_marker_%' AND query NOT LIKE '%pg_stat_statements%'",
    )
    .unwrap();
    assert_eq!(
        tracked, "0",
        "setup statements must not consume pgss entries"
    );
}

#[test]
#[ignore = "needs docker"]
fn failed_startup_reports_container_logs_quickly() {
    let dir = common::scratch_dir("failstart");
    let log = dir.join("log");
    let dataset = dir.join("dataset");
    fs::create_dir_all(&log).unwrap();
    fs::create_dir_all(&dataset).unwrap();
    let name = common::unique("failstart");
    let started = Instant::now();
    // A uid missing from the mounted /etc/passwd makes initdb exit immediately.
    let err = PgContainer::start(&RunSpec {
        name: &name,
        image: "postgres:17",
        label: "pgloupe.synth.test=failstart",
        uid: 54321,
        gid: 54321,
        log_dir: &log,
        dataset_dir: &dataset,
    })
    .unwrap_err();
    let message = format!("{err:#}");
    assert!(
        message.contains("exited before becoming ready"),
        "{message}"
    );
    assert!(
        message.contains("initdb") || message.contains("user"),
        "logs missing: {message}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "took {:?}",
        started.elapsed()
    );
    let filter = format!("name={name}");
    assert_eq!(
        docker(&["ps", "-aq", "--filter", filter.as_str()]).unwrap(),
        ""
    );
}
