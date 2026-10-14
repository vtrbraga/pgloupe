#![expect(clippy::unwrap_used, reason = "test helpers panic on setup failure")]

mod common;

use std::{fs, os::unix::fs::MetadataExt as _};

use synth::container::{PgContainer, RunSpec, docker, remove_stale};

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
