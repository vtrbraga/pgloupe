# pgloupe

> A close look at your Postgres logs and query stats.

**Status:** pre-alpha. `pgloupe` is a working name.

pgloupe is a single-binary, offline analyzer for PostgreSQL that reads server logs and, when available, `pg_stat_statements` snapshots, and joins both into one report.

## Development

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
cargo run -p pgloupe -- --help
```

Building needs a C compiler and `libclang` (the `pg_query` crate compiles Postgres's parser and generates its bindings): on Debian/Ubuntu `sudo apt install build-essential libclang-dev`.

Sanitized test fixtures live in `fixtures/`. Raw logs and stats exports are never committed.

## Stats diff

Compare two `pg_stat_statements` exports (see `docs/data-export.md` in the planning folder for the `psql` commands):

```bash
cargo run -p pgloupe -- stats diff before.csv after.csv
cargo run -p pgloupe -- stats diff before.csv after.csv \
    --before-info before_info.csv --after-info after_info.csv --by mean --min-calls 5
cargo run -p pgloupe -- stats diff before.csv after.csv --format json
```

The header states the window and whether the counters were reset or entries evicted before any number. With the `pg_stat_statements_info` files (or PG17 `stats_since`) a reset is proven; without them it is only inferred. Across a reset the values are the activity since the reset (a lower bound), flagged `partial`. Snapshots from different Postgres major versions are refused.

## Logs dump

Decode a Postgres log (csvlog, jsonlog or stderr; plain or gzip) and print one JSON record per line:

```bash
cargo run -p pgloupe -- logs dump postgresql.csv.gz
cargo run -p pgloupe -- logs dump postgresql.log --prefix '%t:%r:%u@%d:[%p]:'
cargo run -p pgloupe -- logs dump postgresql.log --prefix '%m [%p] ' --log-tz -03 --limit 20
```

The format is detected from the first line; stderr logs need `--prefix` with the server's `log_line_prefix`. A summary (`decoded N records from M lines; K skipped (first at line L: reason)`) goes to stderr, and the exit code is 1 when a non-empty input yields no records. Timezone abbreviations other than `UTC`/`GMT` (for example `BRT`) are ambiguous; pass `--log-tz` with a numeric offset.

## Logs analyze

Summarize one or more logs (csvlog, jsonlog or stderr; plain or gzip): statements grouped by fingerprint with p50/p95/p99, errors, locks and deadlocks, temp files, connections, checkpoints, autovacuum and a timeline.

```bash
cargo run -p pgloupe -- logs analyze postgresql.log.gz
cargo run -p pgloupe -- logs analyze a.csv b.csv --by p95 --top 30 --output json > report.json
cargo run -p pgloupe -- logs analyze server.log --prefix '%t:%r:%u@%d:[%p]:' --threads 4
```

A stderr log without `--prefix` is decoded with an inferred one (printed in the header); if none fits, the command says why and exits 1. The diagnostics block lists skipped line shapes and likely missing server settings (`log_min_duration_statement`, `log_checkpoints`, `lc_messages`, …). Statement durations are exact to the microsecond; percentiles come from a mergeable sketch with 1% relative error.

## Store

`logs ingest` puts logs into a SQLite file once; `logs report` answers any time window from it without reading the logs again.

```bash
cargo run -p pgloupe -- logs ingest --store logs.db postgresql.log.1.gz postgresql.log
cargo run -p pgloupe -- logs report --store logs.db --since 2026-10-14 --until 2026-10-15
```

- Each file is identified by a hash of its decompressed bytes. The same file under another name, or compressed, prints `unchanged`; a log that grew since the last run prints `replaced` (the earlier version is dropped, nothing is counted twice).
- A grown file is read in full again; resuming from a byte offset is not implemented.
- Statistics are stored per UTC hour, so `--since` rounds down and `--until` rounds up to whole hours (the report prints the range it used). `--bucket` must be a multiple of `1m`.
- `--force` re-ingests files already in the store, for example after fixing `--prefix`. A file that decodes to nothing is reported as `failed` and not stored.
- One ingest runs at a time per store (a second one waits 5 s, then reports the store as locked). If a file's time range largely overlaps an earlier input that it does not extend, ingest prints a warning: an older partial copy of a stored log is counted twice.
- Keep the store on the Linux filesystem; SQLite's WAL mode is unreliable on `/mnt/c`.
- With `--max-fingerprints` below the number of distinct statements in an hour, that hour folds the rest into the overflow row, so `report` can differ from `logs analyze` for that run.

## Benchmark input

`cargo run --release --example gen_log -- OUT RECORDS HOURS FINGERPRINTS [SEED]` writes a deterministic jsonlog with many hours and many distinct statements (per tenant and partition), for timing `logs analyze`, `logs ingest` and `logs report` on something larger and wider than the synth corpus.

## Synthetic corpus

`synth` generates Postgres logs and `pg_stat_statements` snapshots from a seeded workload on the Pagila sample database. Requires Docker.

```bash
cargo run -p synth -- generate            # data/synth/17-42/
cargo run -p synth -- generate --pg 16 --seed 7 --only lock_wait,errors --force
```

Output is gitignored. `manifest.json` lists the expected log events per scenario.

Docker-backed tests: `cargo test -p synth -- --ignored`.
