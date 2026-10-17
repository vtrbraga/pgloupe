# Changelog

All notable changes are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Fixed

- `snap`, `export` and `watch` no longer fail with a statement timeout on a large `pg_stat_statements`: its copy gets 10 minutes (wall clock, including the time to send it over the network), every other query keeps 15 seconds. A failed copy no longer leaves a `.part` file behind.
- `doctor` and the settings advice said RDS cannot change `log_line_prefix`. They now recommend the second value RDS for PostgreSQL 16 allows, which carries the application and SQLSTATE, and pgloupe infers that prefix (a log written with it no longer fails with "could not infer the log_line_prefix").

## [0.1.0] - 2026-10-15

The first stable release. It gathers the three alphas (entries below) plus the csvlog record cap and overflow checks in release builds.

### Robustness

- A csvlog record over 16 MiB (usually an unterminated quote) is skipped and counted, and the file stops there, instead of being read whole into memory. Release builds check counters for overflow and stop with an error rather than print a wrapped number.

## [0.1.0-alpha.3] - 2026-10-15

### Report

- Statements get *slower* and *faster* flags (the mean moved by half or more since before the window), spread (standard deviation over the mean) and the slowest run, and the statistics window carries totals: cache hit, file I/O share, planning time, WAL, plus per-database and per-role rollups. New findings: `io_heavy` and `low_cache_hit`.

### Dashboard

- A *Sources* view holds what was read (files, statistics window, `pg_stat_database`); the overview keeps the header to one line. Breakdowns fall back to the statistics when the log carries no databases or users, and skip columns the log never had. Sessions, Locks and Maintenance use the shared activity chart and say "not logged" instead of showing empty panels. Checkpoint tables put sync time first and explain a flat write time. The second row of overview cards no longer overflows at 1400 px, and sparklines no longer sit over the card label. The settings view says how to replace inferred advice with real settings.

### Robustness

- A log line, or one entry assembled from continuation lines, over 16 MiB is skipped and counted instead of being read whole, in the stderr, jsonlog and pgBouncer decoders.

## [0.1.0-alpha.2] - 2026-10-15

Report content from a review of two reports, one over a managed-Postgres capture where the logs caught little.

### Report

- **Server load from `pg_stat_database`.** `snap` writes it beside each snapshot; `report` reads two of them (`--before-db`/`--after-db`, or the files next to the snapshots; `watch` and `export` do too). New section and findings: average running, idle-in-transaction and connected sessions, rollback and cache-hit ratios, abandoned sessions, temp files, deadlocks, and how much of the active time `pg_stat_statements` accounts for.
- **Eviction counts were wrong.** The `dealloc` counter counts eviction rounds (about 5% of `pg_stat_statements.max` each), not entries. The report says rounds and an estimate of entries, names utility statements (`track_utility = off`) when they dominate the churn, and stops calling churn news right after a reset.
- **Statements** gain cache hit, I/O share of execution time, blocks read, rows per call, blocks read per row and WAL per call, a *Top by* selector, and the report keeps the leaders on each of those axes.
- Connection attempts rejected before login are a low-priority note, not the only action. The share of each statement kind adds up to 100%. The threshold finding no longer fires when the log recorded no statements.

### Dashboard

- The overview opens with health and load KPIs that say "not logged" instead of 0 when a setting is off, one line per action, a top-five table and a kinds table that filters Statements, and no empty panels.
- The activity chart has one shared time axis, areas for rates, ticks for events, rows only where there is data, and states what each bar covers.
- Phone layout: the top bar stacks. Zero Plans and Nested calls facts are hidden.

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
