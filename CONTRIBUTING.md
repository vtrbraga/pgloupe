# Contributing to pgloupe

Thanks for looking. Bug reports with a few log lines that show the problem are the most useful
thing you can send; code is welcome too. Everyone taking part follows the [code of conduct](CODE_OF_CONDUCT.md).

## Before you start

- Open an issue first for anything bigger than a fix, so we agree on the shape.
- Keep logs, query text and names out of issues and fixtures. `pgloupe ... --anonymize` rewrites
  names and literals; fixtures in this repository are invented.

## Building

You need a Rust toolchain (the minimum is in `Cargo.toml` as `rust-version`) and `libclang`
(`apt install libclang-dev` or the Xcode command line tools), which `pg_query` uses to build.

```bash
cargo build
cargo test
```

Docker tests start real PostgreSQL servers and are off by default:

```bash
cargo run -p synth -- generate --pg 17      # writes data/synth/17-42 (git-ignored)
cargo test -- --ignored                     # corpus and live-server tests
```

Dashboard tests run under Node 20 (`node --test`); the headless-browser check runs when
`PGLOUPE_CHROME` points at a headless Chrome binary.

## Before you push

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

CI runs the same, plus MSRV, `cargo deny`, `typos`, the generated command reference, the
PostgreSQL 14-18 corpus matrix, the docs build and coverage.

## How the code is laid out

Logs flow through one pipeline, and each stage only talks to its neighbours:

```
source -> decoder -> assembler -> classifier -> accumulators -> store -> outputs
```

- `crates/pgloupe/src/log` decodes stderr, csvlog, jsonlog and pgBouncer logs into records,
  including translated messages (`locales.tsv`).
- `crates/pgloupe/src/analyze` turns records into events and mergeable statistics; `anonymize`
  and `filter` live here because they run while reading.
- `crates/pgloupe/src/stats` diffs `pg_stat_statements` and `pg_stat_database` snapshots.
- `crates/pgloupe/src/store` keeps statistics in SQLite so windows can be reported without
  re-reading logs.
- `crates/pgloupe/src/combined` joins the sources, computes findings and advice, and renders
  text, JSON, CSV and HTML (`templates/` holds the dashboard's HTML, CSS and JavaScript).
- `crates/pgloupe/src/collect` reads a live server (`snap`, `doctor`, `export`, `watch`) and
  fetches logs; one file per vendor under `provider/`.
- `crates/synth` generates a corpus from a real PostgreSQL in Docker, and a benchmark load with
  ground truth (`synth bench`).
- `crates/xtask` generates `docs/src/reference/commands.md` from the CLI definition. CI fails
  if it is stale; after changing help text run `cargo run -p xtask -- generate`.
- `docs/` is the mdBook behind the published docs (`mdbook build docs`). Everything else that
  a user reads is `README.md` and the help text in `crates/pgloupe/src/cli.rs`.

## Style

- Lints are in the root `Cargo.toml`. Do not weaken them; a justified exception is a local
  `#[expect(lint, reason = "...")]`.
- Smallest design that does the job: no option, trait or abstraction nobody uses yet.
- A behaviour change comes with a test that fails without it. Prefer a fixture or a generator
  scenario that produces the real case over a hand-built value.
- Commit messages follow Conventional Commits: `type(scope): subject`, imperative, 50 characters
  or fewer, a body only when the reason is not obvious.

## Licence

By contributing you agree that your work is licensed under the terms of the project
([MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option).

## Changelog

`CHANGELOG.md` follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Every pull request that changes what a user sees (a report, a flag, a message, an install channel) adds a line under **Unreleased**, in the words a user would use: what changed and why they care, not which function moved. Changes that are not user-visible (tests, refactors, CI) need no entry; tick "no changelog needed" in the pull request template. At release time the *Unreleased* heading becomes the version and date.
