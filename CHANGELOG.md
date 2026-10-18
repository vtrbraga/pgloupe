# Changelog

All notable changes are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Changed

- Faster log reading. On synthetic stderr logs of 2 GiB (16 logical cores), against 0.1.1:
  - 8 files, 16 threads: 11.7 s to 1.9 s. Several files are now read at the same time.
  - 1 file, 16 threads: 11.8 s to 4.4 s, and one thread 17.5 s to 12.3 s. When a stderr log is one file, or a few files among many threads, the idle threads split and parse its lines while one thread groups them into entries in order.
  - 8 files with 100,000 distinct statements, 16 threads: 23.0 s to 3.7 s, and one thread 60 s to 23 s. The statement cache is now shared by all threads and holds 262,144 shapes instead of 65,536 per thread.
- Memory no longer grows with the number of threads. With 100,000 distinct statements, 16 threads used 2.3 GiB and now use 0.6 GiB (one thread: 0.45 GiB): the workers keep each statement's figures once, together, instead of one copy each.
- New guide, "Large logs and threads": on the 10.7 GB corpus 4 threads take 16.8 s against 53.4 s for 0.1.1, and `--threads 4` is the recommended setting. It becomes the default in 0.2.0; 0.1.x keeps using every core.
- `logs fetch --provider aws-rds` downloads up to 4 files at the same time (`--jobs`, 1 to 16) and prints a line as each file finishes (`--quiet` hides them). RDS returns about 760 KB per call and each call took 5 to 7 s, one file after the other, so a day of busy logs took close to an hour. A failed file stops the ones after it; the error shown is the first in the list, and the files that finished stay saved.
- `logs fetch --provider aws-rds --since` no longer downloads the hourly file that ended before that time.
- The reports are unchanged, and `--threads 1` still uses one thread. `RAYON_NUM_THREADS` still sets the default number of threads.

## [0.1.1] - 2026-10-17

A patch release of fixes found on a real Amazon RDS PostgreSQL 16 server. Two warnings change: the `lc_messages` one fires less, and the new `%q` one has no minimum line count, so it can appear on logs that were silently skipping lines before.

### Fixed

- A `log_line_prefix` without `%q` that has session fields (`%u`, `%d`, `%a`) now gets a warning saying to put `%q` before them, because background processes print nothing after it and their lines were skipped silently. Checkpoints are no longer reported as "not seen" (a `log_checkpoints` problem) when their lines were skipped, and the `lc_messages` warning no longer fires for lines whose severity label is one pgloupe knows (English, or a built-in language).
- `snap`, `export` and `watch` no longer fail with a statement timeout on a large `pg_stat_statements`: its copy gets 10 minutes (wall clock, including the time to send it over the network), every other query keeps 15 seconds. A failed copy no longer leaves a `.part` file behind.
- `doctor` and the settings advice said RDS cannot change `log_line_prefix`. They now recommend the second value RDS for PostgreSQL 16 allows, which carries the application and SQLSTATE, and pgloupe infers that prefix (a log written with it no longer fails with "could not infer the log_line_prefix").
- A client with an IPv6 address in `%r` (`2600:1f18::1(5432)`) is read as that host and port, not as the text before its first colon. This broke the user and database of every such line under both RDS prefixes. `%h` followed by a colon in the prefix is not covered (see the formats reference).

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
