# Changelog

All notable changes are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

## [0.1.0-alpha.1] - 2026-10-15

The first pre-release. It exists to prove the release pipeline (archives, installers, Homebrew, crates.io, Docker) and to get the tool in front of real servers. Formats are not stable: the JSON schema and the store schema can change before 0.1.0.

### Reports

- One report from stderr, csvlog or jsonlog files (plain or gzip, zstd, xz, lz4, bzip2) and, optionally, two `pg_stat_statements` snapshots, as HTML, JSON, text or CSV.
- *What to fix first*: ranked problems with evidence and advice; *Settings*: which server settings limit the report and what to change.
- Per-statement percentiles, parse/bind/fetch time, errors by SQLSTATE and shape, lock waits with the statement that held the lock, temp files, checkpoints, autovacuum, sessions and abrupt endings, schema changes.
- Comparison of a period with the one before it, from a store.
- Filters by attribute, pattern, time of day and weekday; `--anonymize` at read time.

### Collecting

- `snap`, `doctor`, `logs fetch`, `export` and `watch` for a live server; Amazon RDS first, other clouds experimental.
- A live dashboard that keeps your view across refreshes.

### Experimental

- pgBouncer logs; servers that log in Portuguese (Brazil), Spanish, French or German.

### Distribution

- Archives for Linux and macOS (x86_64, aarch64) and Windows (x86_64, experimental), shell and PowerShell installers, `pgloupe-update`, a Homebrew formula, a Docker image on ghcr.io, man pages and shell completions.
