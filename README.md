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

## Synthetic corpus

`synth` generates Postgres logs and `pg_stat_statements` snapshots from a seeded workload on the Pagila sample database. Requires Docker.

```bash
cargo run -p synth -- generate            # data/synth/17-42/
cargo run -p synth -- generate --pg 16 --seed 7 --only lock_wait,errors --force
```

Output is gitignored. `manifest.json` lists the expected log events per scenario.

Docker-backed tests: `cargo test -p synth -- --ignored`.
