#![expect(clippy::unwrap_used, reason = "test helpers panic on setup failure")]

mod common;

use std::path::PathBuf;

use synth::{
    bench::{run, truth::Truth},
    cli::{BenchRunArgs, PgMajor},
};

#[test]
#[ignore = "needs docker; takes about a minute"]
fn a_small_run_reaches_its_target_and_writes_the_truth() {
    let out = common::scratch_dir("bench");
    let args = BenchRunArgs {
        pg: PgMajor::V17,
        seed: 1,
        target_size: "64M".to_owned(),
        clients: 4,
        scale: 1,
        snapshot_secs: 5,
        checkpoint_secs: 10,
        force: true,
        out,
        cache: PathBuf::from("../../data/cache"),
    };
    let dir = run::run(&args).unwrap();
    let truth = Truth::read(&dir).unwrap();
    assert!(truth.reached_bytes >= truth.target_bytes);
    assert!(truth.delta.commits > 0);
    assert!(truth.transactions() > 0);
    assert!(truth.snapshots.after_reset.is_some());
    for format in ["log", "csv", "json"] {
        assert!(truth.formats[format].bytes > 0, "{format}");
    }
    for worker in [
        "tpcb", "literal", "locks", "analytic", "errors", "churn", "mixed",
    ] {
        assert!(truth.workers.iter().any(|w| w.name == worker), "{worker}");
    }
    assert_eq!(
        u64::try_from(truth.delta.deadlocks).unwrap(),
        truth.event("locks", "deadlocks"),
        "the sessions saw every deadlock the server counted"
    );
    assert!(truth.event("errors", "errors") > 0);
    assert!(truth.event("churn", "abrupt") > 0);
    assert!(dir.join("snapshots/pgss_0.csv").exists());
    assert!(
        dir.join(format!("snapshots/pgss_{}.csv", truth.snapshots.last))
            .exists()
    );
}
