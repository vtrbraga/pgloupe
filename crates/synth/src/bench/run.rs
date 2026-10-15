//! `bench run`: workers, ticker and stop condition around one container.

use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, bail};

use super::{
    SETTINGS, oltp, pgbench, script, size,
    truth::{self, Snapshots, Truth, WorkerStats},
};
use crate::{
    bootstrap,
    cli::BenchRunArgs,
    container::{PgContainer, RunSpec},
    dataset,
    session::Target,
    snapshot,
};

/// Seconds one pgbench round runs; the stop flag is looked at between rounds.
const ROUND_SECS: u32 = 10;

/// How often the main thread looks at the log size.
const POLL: Duration = Duration::from_secs(5);

/// Share of the target (in tenths) at which `pg_stat_statements_reset()` runs.
const RESET_AT_TENTHS: u64 = 6;

#[expect(clippy::print_stderr, reason = "progress is reported on the terminal")]
fn say(message: &str) {
    eprintln!("{message}");
}

/// Runs the load until the logs reach the target size and writes `bench-truth.json`.
///
/// Returns the run directory.
///
/// # Errors
///
/// Fails on Docker, database or I/O errors, or when a worker fails.
pub fn run(args: &BenchRunArgs) -> anyhow::Result<PathBuf> {
    let target_bytes = args.target_bytes()?;
    let dir = args.run_dir();
    if dir.exists() {
        if !args.force {
            bail!("{} exists; use --force to replace it", dir.display());
        }
        fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
    }
    let (pg, target, log_dir, snap_dir) = start(args, &dir)?;
    let mut control = target.control("postgres")?;
    tune(&mut control)?;
    target
        .control("bench")?
        .batch_execute(oltp::SETUP_SQL)
        .context("creating the hot table")?;
    let scripts = script::write_scripts(&dir.join("scripts"))?;
    pgbench::copy_scripts(&pg, &dir.join("scripts"))?;

    let major = args.pg.number();
    let start = truth::read(&mut control, major)?;
    snapshot::take(&mut control, &snap_dir, 0)?;

    let tpcb_clients = (args.clients / 2).max(1);
    let literal_clients = args.clients.saturating_sub(tpcb_clients).max(1);
    let tpcb_args = pgbench::tpcb_args(tpcb_clients, ROUND_SECS);
    let literal_args = pgbench::literal_args(literal_clients, ROUND_SECS, args.scale, &scripts);

    let stop = AtomicBool::new(false);
    let began = Instant::now();
    let (workers, ticker) = thread::scope(|scope| {
        let handles = vec![
            scope.spawn(|| {
                worker("tpcb", &stop, || {
                    pgbench::run_round(&pg, "bench:tpcb", &tpcb_args)
                })
            }),
            scope.spawn(|| {
                worker("literal", &stop, || {
                    pgbench::run_round(&pg, "bench:literal", &literal_args)
                })
            }),
            scope.spawn(|| oltp::locks(target, args.seed, &stop)),
            scope.spawn(|| oltp::analytic(target, &stop)),
            scope.spawn(|| oltp::errors(target, &stop)),
            scope.spawn(|| oltp::churn(target, &stop)),
            scope.spawn(|| oltp::mixed(target, &stop)),
        ];
        let ticker = scope.spawn(|| {
            ticker(
                &Ticker {
                    target,
                    snap_dir: &snap_dir,
                    log_dir: &log_dir,
                    target_bytes,
                    snapshot_every: Duration::from_secs(args.snapshot_secs),
                    checkpoint_every: Duration::from_secs(args.checkpoint_secs),
                },
                &stop,
            )
        });
        watch(&log_dir, target_bytes, &stop, began);
        let workers: Vec<_> = handles.into_iter().map(join).collect();
        (workers, join(ticker))
    });
    let workers = workers.into_iter().collect::<anyhow::Result<Vec<_>>>()?;
    let ticker = ticker?;

    let last = ticker
        .last
        .checked_add(1)
        .context("more than 255 snapshots")?;
    snapshot::take(&mut control, &snap_dir, last)?;
    let end = truth::read(&mut control, major)?;
    drop(control);
    pg.stop()?;

    let formats = size::per_format(&log_dir)?;
    let gzipped = gzip_oldest(&log_dir)?;
    let reached_bytes = formats.values().map(|f| f.bytes).min().unwrap_or(0);
    Truth {
        pg_major: major,
        seed: args.seed,
        scale: args.scale,
        target_bytes,
        reached_bytes,
        formats,
        wall_secs: began.elapsed().as_secs(),
        delta: end.minus(start),
        dealloc_at_end: end.dealloc,
        workers,
        snapshots: Snapshots {
            first: 0,
            after_reset: ticker.after_reset,
            last,
        },
        gzipped,
    }
    .write(&dir)?;
    drop(pg);
    Ok(dir)
}

/// Starts the container with the bench settings and loads the data.
fn start(
    args: &BenchRunArgs,
    dir: &Path,
) -> anyhow::Result<(PgContainer, Target, PathBuf, PathBuf)> {
    let dataset_dir = fs::canonicalize(dataset::ensure_pagila(&args.cache)?)
        .context("resolving dataset directory")?;
    let log_dir = dir.join("log");
    let snap_dir = dir.join("snapshots");
    for d in [&log_dir, &snap_dir] {
        fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
    }
    let log_dir = fs::canonicalize(&log_dir).context("resolving the log directory")?;
    let owner = fs::metadata(&log_dir).context("reading the log directory owner")?;

    let image = format!("postgres:{}", args.pg.number());
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let name = format!("pgloupe-bench-{}-{nanos}", process::id());
    let pg = PgContainer::start(&RunSpec {
        name: &name,
        image: &image,
        pg_major: args.pg.number(),
        label: super::LABEL,
        uid: owner.uid(),
        gid: owner.gid(),
        log_dir: &log_dir,
        dataset_dir: &dataset_dir,
        overrides: SETTINGS,
    })?;
    let target = Target { port: pg.port() };
    bootstrap::load(&pg, target)?;
    say("loading pgbench data");
    let scale = args.scale.to_string();
    pg.exec_unlogged(&[
        "pgbench", "-i", "-q", "-s", &scale, "-U", "postgres", "bench",
    ])
    .context("initializing pgbench")?;

    Ok((pg, target, log_dir, snap_dir))
}

/// Compresses the oldest file of every format that rotated, so a run reads both plain and gzip
/// input. Returns the names of the compressed files.
fn gzip_oldest(log_dir: &Path) -> anyhow::Result<Vec<String>> {
    let mut done = Vec::new();
    for ext in ["log", "csv", "json"] {
        let mut files: Vec<PathBuf> = fs::read_dir(log_dir)
            .with_context(|| format!("reading {}", log_dir.display()))?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some(ext))
            .collect();
        files.sort();
        if files.len() < 2 {
            continue;
        }
        let status = process::Command::new("gzip")
            .arg("-f")
            .arg(&files[0])
            .status()
            .context("running gzip")?;
        if !status.success() {
            bail!("gzip failed on {}", files[0].display());
        }
        if let Some(name) = files[0].file_name().and_then(|n| n.to_str()) {
            done.push(name.to_owned());
        }
    }
    Ok(done)
}

/// Autovacuum on and chatty, so its entries appear in the log without a hand-run `VACUUM`.
fn tune(control: &mut postgres::Client) -> anyhow::Result<()> {
    // One statement per call: ALTER SYSTEM cannot run inside a transaction block.
    for sql in [
        "ALTER SYSTEM SET autovacuum = on",
        "ALTER SYSTEM SET autovacuum_naptime = '5s'",
        "ALTER SYSTEM SET autovacuum_vacuum_scale_factor = 0.02",
        "ALTER SYSTEM SET log_autovacuum_min_duration = 0",
        "SELECT pg_reload_conf()",
    ] {
        control
            .batch_execute(sql)
            .with_context(|| format!("running {sql}"))?;
    }
    Ok(())
}

/// Runs `round` until `stop`; a failed round stops everyone and returns its error.
fn worker(
    name: &str,
    stop: &AtomicBool,
    mut round: impl FnMut() -> anyhow::Result<u64>,
) -> anyhow::Result<WorkerStats> {
    let mut stats = WorkerStats {
        name: name.to_owned(),
        rounds: 0,
        transactions: 0,
        events: BTreeMap::new(),
    };
    while !stop.load(Ordering::Relaxed) {
        match round() {
            Ok(transactions) => {
                stats.rounds += 1;
                stats.transactions += transactions;
            }
            Err(e) => {
                stop.store(true, Ordering::Relaxed);
                return Err(e.context(format!("worker {name}")));
            }
        }
    }
    Ok(stats)
}

/// Polls the log size and sets `stop` at the target (or returns when a worker already did).
fn watch(log_dir: &Path, target_bytes: u64, stop: &AtomicBool, began: Instant) {
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(POLL);
        match size::log_bytes(log_dir) {
            Ok(bytes) => {
                say(&format!(
                    "{:>5}s  {} / {} MiB",
                    began.elapsed().as_secs(),
                    bytes >> 20,
                    target_bytes >> 20
                ));
                if bytes >= target_bytes {
                    stop.store(true, Ordering::Relaxed);
                }
            }
            Err(e) => {
                say(&format!("cannot read the log directory: {e:#}"));
                stop.store(true, Ordering::Relaxed);
            }
        }
    }
}

/// Joins a scoped thread, turning a panic into an error.
fn join<T>(handle: thread::ScopedJoinHandle<'_, anyhow::Result<T>>) -> anyhow::Result<T> {
    handle
        .join()
        .unwrap_or_else(|_| Err(anyhow::anyhow!("a bench thread panicked")))
}

struct Ticker<'a> {
    target: Target,
    snap_dir: &'a Path,
    log_dir: &'a Path,
    target_bytes: u64,
    snapshot_every: Duration,
    checkpoint_every: Duration,
}

/// Snapshot numbers the ticker wrote.
struct Ticked {
    last: u8,
    after_reset: Option<u8>,
}

/// Periodic snapshots and checkpoints, and one statistics reset at 60% of the target.
fn ticker(cfg: &Ticker<'_>, stop: &AtomicBool) -> anyhow::Result<Ticked> {
    let result = tick_loop(cfg, stop);
    if result.is_err() {
        stop.store(true, Ordering::Relaxed);
    }
    result
}

fn tick_loop(cfg: &Ticker<'_>, stop: &AtomicBool) -> anyhow::Result<Ticked> {
    let mut control = cfg.target.control("postgres")?;
    let mut ticked = Ticked {
        last: 0,
        after_reset: None,
    };
    let reset_at = cfg.target_bytes / 10 * RESET_AT_TENTHS;
    let (mut next_snapshot, mut next_checkpoint) = (Instant::now(), Instant::now());
    next_snapshot += cfg.snapshot_every;
    next_checkpoint += cfg.checkpoint_every;
    let mut out_of_numbers = false;
    let mut reset_done = false;
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(500));
        let now = Instant::now();
        if now >= next_checkpoint {
            control.batch_execute("CHECKPOINT")?;
            next_checkpoint = now + cfg.checkpoint_every;
        }
        if !reset_done && size::log_bytes(cfg.log_dir)? >= reset_at {
            control.batch_execute("SELECT pg_stat_statements_reset()")?;
            reset_done = true;
            ticked.after_reset = take_next(&mut control, cfg, &mut ticked)?;
        } else if now >= next_snapshot {
            if take_next(&mut control, cfg, &mut ticked)?.is_none() && !out_of_numbers {
                out_of_numbers = true;
                say("255 snapshots written; no more periodic snapshots");
            }
            next_snapshot = now + cfg.snapshot_every;
        }
    }
    Ok(ticked)
}

/// Takes the next numbered snapshot; `None` when all 255 numbers are used.
fn take_next(
    control: &mut postgres::Client,
    cfg: &Ticker<'_>,
    ticked: &mut Ticked,
) -> anyhow::Result<Option<u8>> {
    // The final snapshot needs the number after the last one, so one number stays free.
    let Some(n) = ticked.last.checked_add(1).filter(|n| *n < u8::MAX) else {
        return Ok(None);
    };
    snapshot::take(control, cfg.snap_dir, n)?;
    ticked.last = n;
    Ok(Some(n))
}
