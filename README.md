# pgloupe

> A close look at your Postgres logs and query stats.

**Status:** pre-alpha. `pgloupe` is a working name.

pgloupe is a single-binary, offline analyzer for PostgreSQL that reads server logs and, when available, `pg_stat_statements` snapshots, and joins both into one report.

## Development

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
cargo run -- --help
```

Sanitized test fixtures live in `fixtures/`. Raw logs and stats exports are never committed.
