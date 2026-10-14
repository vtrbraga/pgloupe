# pgloupe

> A close look at your Postgres logs and query stats.

**Status:** pre-alpha. `pgloupe` is a working name.

pgloupe is a single-binary, offline analyzer for PostgreSQL that reads server logs and, when available, `pg_stat_statements` snapshots, and joins both into one report.

## Development

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
cargo run -p pgloupe -- --help
```

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

## Synthetic corpus

`synth` generates Postgres logs and `pg_stat_statements` snapshots from a seeded workload on the Pagila sample database. Requires Docker.

```bash
cargo run -p synth -- generate            # data/synth/17-42/
cargo run -p synth -- generate --pg 16 --seed 7 --only lock_wait,errors --force
```

Output is gitignored. `manifest.json` lists the expected log events per scenario.

Docker-backed tests: `cargo test -p synth -- --ignored`.
