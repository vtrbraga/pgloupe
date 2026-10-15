//! The `generate` pipeline: container, dataset, scenarios, snapshots, manifest.

use std::{
    fs,
    os::unix::fs::MetadataExt as _,
    path::PathBuf,
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, bail};

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

#[expect(clippy::print_stderr, reason = "progress is reported on the terminal")]
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
        check_reusable(&Manifest::read(&final_dir)?, scenarios)?;
        eprintln!("{} exists; use --force to regenerate", final_dir.display());
        return Ok(final_dir);
    }
    let dataset_dir = fs::canonicalize(dataset::ensure_pagila(&args.cache)?)
        .context("resolving dataset directory")?;

    let tmp = final_dir.with_extension("tmp");
    if tmp.exists() {
        fs::remove_dir_all(&tmp).with_context(|| format!("removing {}", tmp.display()))?;
    }
    let log_dir = tmp.join("log");
    fs::create_dir_all(&log_dir).with_context(|| format!("creating {}", log_dir.display()))?;
    let log_dir =
        fs::canonicalize(&log_dir).with_context(|| format!("resolving {}", log_dir.display()))?;
    let owner = fs::metadata(&log_dir)
        .with_context(|| format!("reading owner of {}", log_dir.display()))?;

    let image = format!("postgres:{}", args.pg.number());
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let name = format!("pgloupe-synth-{}-{nanos}", process::id());
    let pg = PgContainer::start(&RunSpec {
        name: &name,
        image: &image,
        pg_major: args.pg.number(),
        label: LABEL,
        uid: owner.uid(),
        gid: owner.gid(),
        log_dir: &log_dir,
        dataset_dir: &dataset_dir,
        overrides: &[],
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
        settings: container::settings(args.pg.number())
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
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

/// Fails unless `existing` was generated for exactly the requested scenarios.
fn check_reusable(existing: &Manifest, scenarios: &[&Scenario]) -> anyhow::Result<()> {
    let requested: Vec<&str> = scenarios.iter().map(|s| s.name).collect();
    let generated: Vec<&str> = existing.scenarios.iter().map(String::as_str).collect();
    if generated != requested {
        bail!(
            "existing corpus was generated for scenarios [{}], not [{}]; --force replaces the existing corpus",
            generated.join(", "),
            requested.join(", ")
        );
    }
    Ok(())
}

fn run_phase(ctx: &mut Ctx, scenarios: &[&Scenario], phase: Phase) -> anyhow::Result<()> {
    for scenario in scenarios.iter().filter(|s| s.phase == phase) {
        ctx.begin(scenario);
        (scenario.run)(ctx).with_context(|| format!("scenario {}", scenario.name))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[expect(clippy::unnecessary_wraps, reason = "must match Scenario::run")]
    fn noop(_: &mut Ctx) -> anyhow::Result<()> {
        Ok(())
    }

    const A: Scenario = Scenario {
        name: "a",
        phase: Phase::Main,
        run: noop,
        expect: &[],
    };
    const B: Scenario = Scenario {
        name: "b",
        phase: Phase::Main,
        run: noop,
        expect: &[],
    };

    fn manifest_with(scenarios: &[&str]) -> Manifest {
        Manifest {
            generator_version: String::new(),
            pg_major: 17,
            server_version_num: 170_000,
            image: String::new(),
            image_digest: String::new(),
            seed: 42,
            pagila_commit: String::new(),
            settings: Vec::new(),
            load_finished_at: String::new(),
            snapshots_at: Vec::new(),
            scenarios: scenarios.iter().map(|s| (*s).to_owned()).collect(),
            multiline_texts: Vec::new(),
            expect: Vec::new(),
        }
    }

    #[test]
    fn reuse_accepts_the_same_scenarios() {
        check_reusable(&manifest_with(&["a", "b"]), &[&A, &B]).unwrap();
    }

    #[test]
    fn reuse_rejects_a_corpus_made_for_other_scenarios() {
        let err = check_reusable(&manifest_with(&["a"]), &[&A, &B])
            .unwrap_err()
            .to_string();
        assert!(err.contains("--force replaces"), "{err}");
        assert!(err.contains('a') && err.contains('b'), "{err}");
    }
}
