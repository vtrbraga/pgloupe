//! The `generate` pipeline: container, dataset, scenarios, snapshots, manifest.

use std::{
    fs,
    os::unix::fs::MetadataExt as _,
    path::PathBuf,
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context as _;

use crate::{
    bootstrap,
    cli::GenerateArgs,
    container::{self, LABEL, PgContainer, RunSpec},
    dataset::{self, PAGILA_COMMIT},
    manifest::{self, Manifest},
    scenario::{self, Ctx, Phase, Scenario},
    session::{Target, query_scalar},
    snapshot,
};

/// CLI entry: removes stale containers, then runs the selected scenarios.
///
/// Returns the run directory.
///
/// # Errors
///
/// Fails on unknown scenario names, Docker, network, database or I/O errors.
pub fn run(args: &GenerateArgs) -> anyhow::Result<PathBuf> {
    let scenarios = scenario::select(&args.only)?;
    container::remove_stale(LABEL)?;
    run_with(args, &scenarios)
}

/// Generates a corpus for exactly `scenarios`, without touching other containers.
///
/// Returns the run directory. Reuses a complete existing corpus unless `args.force`.
///
/// # Errors
///
/// Fails on Docker, network, database or I/O errors; partial output stays in `<dir>.tmp`.
pub fn run_with(args: &GenerateArgs, scenarios: &[&Scenario]) -> anyhow::Result<PathBuf> {
    let final_dir = args.run_dir();
    if final_dir.join(manifest::FILE_NAME).exists() && !args.force {
        eprintln!("{} exists; use --force to regenerate", final_dir.display());
        return Ok(final_dir);
    }
    let dataset_dir = fs::canonicalize(dataset::ensure_pagila(&args.cache)?)?;

    let tmp = final_dir.with_extension("tmp");
    if tmp.exists() {
        fs::remove_dir_all(&tmp).with_context(|| format!("removing {}", tmp.display()))?;
    }
    let log_dir = tmp.join("log");
    fs::create_dir_all(&log_dir).with_context(|| format!("creating {}", log_dir.display()))?;
    let log_dir = fs::canonicalize(&log_dir)?;
    let owner = fs::metadata(&log_dir)?;

    let image = format!("postgres:{}", args.pg.number());
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let name = format!("pgloupe-synth-{}-{nanos}", process::id());
    let pg = PgContainer::start(&RunSpec {
        name: &name,
        image: &image,
        label: LABEL,
        uid: owner.uid(),
        gid: owner.gid(),
        log_dir: &log_dir,
        dataset_dir: &dataset_dir,
    })?;
    let target = Target { port: pg.port() };

    let load_finished_at = bootstrap::load(&pg, target)?;
    let mut control = target.control("postgres")?;
    let mut ctx = Ctx::new(target, args.seed);
    let mut snapshots_at = vec![snapshot::take(&mut control, &tmp, 0)?];
    run_phase(&mut ctx, scenarios, Phase::Main)?;
    snapshots_at.push(snapshot::take(&mut control, &tmp, 1)?);
    run_phase(&mut ctx, scenarios, Phase::Churn)?;
    snapshots_at.push(snapshot::take(&mut control, &tmp, 2)?);

    let server_version_num = query_scalar(&mut control, "SHOW server_version_num")?
        .parse()
        .context("server_version_num is not an integer")?;
    drop(control);
    pg.stop()?;

    let manifest = Manifest {
        generator_version: env!("CARGO_PKG_VERSION").to_owned(),
        pg_major: args.pg.number(),
        server_version_num,
        image_digest: container::image_digest(&image)?,
        image,
        seed: args.seed,
        pagila_commit: PAGILA_COMMIT.to_owned(),
        settings: container::SETTINGS
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        load_finished_at,
        snapshots_at,
        scenarios: scenarios.iter().map(|s| s.name.to_owned()).collect(),
        multiline_texts: ctx.multiline_texts,
        expect: scenarios.iter().flat_map(|s| s.expectations()).collect(),
    };
    manifest.write(&tmp)?;
    drop(pg);

    if final_dir.exists() {
        fs::remove_dir_all(&final_dir)
            .with_context(|| format!("removing {}", final_dir.display()))?;
    }
    fs::rename(&tmp, &final_dir).with_context(|| format!("renaming {}", tmp.display()))?;
    Ok(final_dir)
}

fn run_phase(ctx: &mut Ctx, scenarios: &[&Scenario], phase: Phase) -> anyhow::Result<()> {
    for scenario in scenarios.iter().filter(|s| s.phase == phase) {
        ctx.begin(scenario);
        (scenario.run)(ctx).with_context(|| format!("scenario {}", scenario.name))?;
    }
    Ok(())
}
