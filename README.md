# pgloupe

[![CI](https://github.com/vtrbraga/pgloupe/actions/workflows/ci.yml/badge.svg)](https://github.com/vtrbraga/pgloupe/actions/workflows/ci.yml)
[![Coverage](https://codecov.io/gh/vtrbraga/pgloupe/graph/badge.svg)](https://codecov.io/gh/vtrbraga/pgloupe)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)
[![PostgreSQL](https://img.shields.io/badge/PostgreSQL-14%20%7C%2015%20%7C%2016%20%7C%2017%20%7C%2018-336791?logo=postgresql&logoColor=white)](#what-it-reads)
[![Rust](https://img.shields.io/badge/rust-1.88%2B-orange?logo=rust)](https://www.rust-lang.org)
[![Platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20macOS-lightgrey)](#install)

A close look at your Postgres logs and query stats, in one offline binary.

Point it at a server's log files and, if you have them, two `pg_stat_statements` snapshots. It
tells you **what to fix first**, then shows the numbers behind it: slow statements with real
percentiles, errors, lock waits and who blocked whom, temp files, checkpoints, autovacuum,
connections. It also tells you which server settings are limiting the report and what to change.

![The overview of a pgloupe report](https://raw.githubusercontent.com/vtrbraga/pgloupe/main/assets/dashboard-overview.png)

<sub>A report over a generated test server, not real data.</sub>

- **One file in, one file out.** No agent, no extension, no database to host. The HTML report
  is a single file with no network requests; JSON, text and CSV come from the same run.
- **Logs and statistics together.** The log shows what happened (errors, locks, slow runs);
  `pg_stat_statements` counts everything the log never saw, and `pg_stat_database` says how
  loaded the server was. pgloupe joins them per statement and tells you when they disagree.
- **One row per statement.** Statements are grouped by their parse tree, so the same query
  with different literals is one row, and percentiles are per statement.
- **Works on managed Postgres.** It only needs `pg_stat_statements`, which Amazon RDS, Cloud
  SQL, Azure and Aiven already have. It can fetch RDS logs itself.
- **Tells you why a number is odd.** Resets, evictions, windows that do not match, unreadable
  query text, logs without durations: each becomes a plain sentence, not a silent zero.
- **Fast.** On a 10.7 GB set of 40 generated PostgreSQL 17 logs it read about 200 MB/s on one
  thread using about 46 MiB of memory, and about 1.2 GB/s on 16 threads (512 MiB). Memory
  depends on the number of distinct statements, not on log size. Figures vary by hardware.
- **Safe to share.** `--anonymize` rewrites names and literals before anything is counted.

## Install

pgloupe is one binary. Every release has prebuilt archives for Linux (x86-64, arm64), macOS (Intel,
Apple silicon) and Windows (x86-64).

```bash
# Linux, macOS: installer script
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/vtrbraga/pgloupe/releases/latest/download/pgloupe-installer.sh | sh

# Homebrew
brew install vtrbraga/tap/pgloupe

# Cargo: prebuilt, or from source (needs a C toolchain and libclang)
cargo binstall pgloupe
cargo install pgloupe

# Docker
docker run --rm -v "$PWD:/data" ghcr.io/vtrbraga/pgloupe:latest report /data/postgresql.log
```

Windows (PowerShell), archives with checksums and attestations, updating and uninstalling are in
the [install guide](https://vtrbraga.github.io/pgloupe/install.html).

Linux and macOS are tested in CI. Windows builds are experimental.

## Quick start

A report from a log file:

```bash
pgloupe report postgresql.log --output html --out report.html
```

pgloupe finds the `log_line_prefix` by itself and prints what it chose. If it cannot, it says
why instead of printing an empty report.

A report that also uses two `pg_stat_statements` snapshots:

```bash
pgloupe snap --conn postgres://me@db.example.com/postgres --out snaps/    # take one now
# ... wait the time you want to look at, then take another ...
pgloupe snap --conn postgres://me@db.example.com/postgres --out snaps/

pgloupe report postgresql.log \
  --before snaps/pgss_<older>.csv --after snaps/pgss_<newer>.csv \
  --before-info snaps/pgss_info_<older>.csv --after-info snaps/pgss_info_<newer>.csv \
  --output html --out report.html
```

`snap` also writes `pg_stat_database` and the server settings beside each snapshot; `report`
picks them up from there (or take `--before-db`, `--after-db` and `--settings`).

Only the statistics, no logs:

```bash
pgloupe report --before old.csv --after new.csv
```

Keep a live report fresh (it prints a local URL; the page is served on loopback only):

```bash
pgloupe watch --conn postgres://me@db.example.com/postgres \
  --provider aws-rds --instance my-db --interval 5m --window 24h
```

## Amazon RDS

You need the `aws` CLI signed in, a database role that can read statistics, and these settings
in the parameter group (`pgloupe doctor` checks all of them and says what is missing):

| Setting | Why |
|---|---|
| `log_min_duration_statement` (for example `250`) | without durations the log has no slow statements |
| `log_lock_waits = on` | lock waits, and who held the lock |
| `log_temp_files = 0` | temp-file spills |
| `log_checkpoints = on`, `log_autovacuum_min_duration` (for example `1000`) | maintenance |
| `log_connections`, `log_disconnections = on` | sessions and logins |
| `track_io_timing = on` | I/O time in `pg_stat_statements` |
| `pg_stat_statements` in `shared_preload_libraries` | the statistics (a reboot applies it) |

Grant the role `pg_monitor` (or `pg_read_all_stats`) so it can see other users' query text.

```bash
export PGLOUPE_DSN='postgres://me@my-db.abc.us-east-1.rds.amazonaws.com:5432/postgres'  # password: PGPASSWORD or ~/.pgpass
pgloupe doctor --vendor rds
pgloupe export --provider aws-rds --instance my-db --since 24h --out export/ --report report.html
```

`export` checks the server, takes a snapshot, downloads the logs and builds the report in one go.
Other clouds (CloudWatch, Azure, Cloud SQL, Aiven, Heroku) and logs read over SQL are available
as `--provider` values and are experimental: they are tested against recorded payloads, not the
real services.

## What the report shows

| Section | What is in it |
|---|---|
| Overview | Health and load figures (errors, lock waits, deadlocks, temp files, rollbacks, abandoned sessions; running, idle-in-transaction and connected sessions, cache hit), the actions, the top five statements and the statement kinds. "Not logged" where a setting is off |
| What to fix first | Ranked problems, each with the numbers, the advice and the statements involved |
| Statements | Per statement: calls, total, mean, p50, p95, p99, max, spread, errors, lock waits, temp files; parse, bind and cursor-fetch time; cache hit, I/O share, blocks read, rows and WAL per call; *slower* and *faster* flags; the log next to `pg_stat_statements` |
| Errors | By SQLSTATE and message shape, with the statement, DETAIL and HINT |
| Locks and temp files | Waits over time, who held the lock, deadlocks, temp-file spills |
| Sessions | Connections, session durations, connections per host, sessions that ended abruptly |
| Maintenance | Checkpoints (WAL between them, causes), autovacuum per table, schema changes and temporary tables |
| Breakdowns | By database, user, application and host; from the statistics when the log has none |
| Slowest | The slowest single executions, with parameters only if you ask |
| Pooler | pgBouncer logins, session endings and `stats:` lines (experimental) |
| Comparison | One period against the one before it |
| Settings | Which settings limit this report, and what to change |
| Sources | What was read: files, statistics window, `pg_stat_database` totals |
| Data caveats | Why a number may be off |

The HTML report keeps its view in the URL, searches with `/`, and prints cleanly. The same data
is available as JSON (`schema_version` in the file) and the statement table as CSV. The
[docs](https://vtrbraga.github.io/pgloupe/concepts/reading-the-report.html) explain each section.

## Commands

| Command | What it does |
|---|---|
| `report` | One report from log files, a store, and/or two snapshots: `--output text,json,html,csv` |
| `logs analyze` | Summarize log files |
| `logs ingest` / `logs report` | Keep logs in a small SQLite store and report on any time window without re-reading them |
| `logs dump` | Decode a log and print every record as JSON |
| `logs fetch` | Download log files from RDS (and others), resumable |
| `stats diff` | Rank what changed between two `pg_stat_statements` snapshots |
| `snap` | Take a snapshot (and the settings) from a live server; read-only |
| `doctor` | Check the server and the role, and say how to fix what is missing |
| `export` | `doctor`, `snap`, `logs fetch` and `report` in one command |
| `watch` | Loop `snap`, fetch and report, and serve the live dashboard locally |

Useful flags, all of which work on `report`, `logs analyze` and `logs ingest`:

- `--database`, `--user`, `--application`, `--host` (and `--exclude-…`): exact names, globs
  (`pgAdmin*`) or `re:regex`.
- `--between 09:00-18:00`, `--exclude-hours 5-7`, `--weekdays mon-fri`, `--tz Europe/Paris`,
  `--pid`, `--exclude-message`, `--include-query`, `--exclude-query`.
- `--since`, `--until` and `--baseline previous` on a store report.
- `--anonymize` (with `--anonymize-key` or `PGLOUPE_ANONYMIZE_KEY` for private pseudonyms).
- `--top N|all`, `--slowest N`, `--params` (keeps bind values; they can hold personal data).
- `--display-tz Europe/Paris` shows times in that zone in the HTML report.

Run any command with `--help`, or read the
[command reference](https://vtrbraga.github.io/pgloupe/reference/commands.html). Exit codes:
0 success, 1 the work failed, 2 the command line is wrong.

## What it reads

- **PostgreSQL 14 to 18**, tested against real servers of each version.
- **Logs:** stderr (any `log_line_prefix`, inferred), csvlog and jsonlog, plain or compressed
  with gzip, zstd, xz, lz4 or bzip2. Multi-line entries, application names with spaces and
  Amazon RDS truncation markers are handled.
- **Languages (experimental):** servers that log in Portuguese (Brazil), Spanish, French or
  German are read as if they were English; the language is detected, or give `--lc-messages`.
  `--messages-file` adds another from a table of `locale<TAB>english<TAB>translated`.
- **pgBouncer logs (experimental):** logins, why sessions ended, and the once-a-minute `stats:`
  line (transactions, queries, wait time) get their own view.
- **Statistics:** CSV exports of `pg_stat_statements`, `pg_stat_statements_info`,
  `pg_stat_database` and `pg_settings`, which `snap` writes.

Details, including what is not read, are in
[supported versions and formats](https://vtrbraga.github.io/pgloupe/reference/formats-and-versions.html).

## Limits

- It reads files you give it or fetch with the commands above; it is not a monitoring system.
- Syslog output and `auto_explain` plans are not read. A log line, entry or csvlog record over
  16 MiB is skipped and counted.
- pgBouncer logs and non-English servers are experimental: tested on generated and invented
  input, not on production ones.
- Non-English servers: autovacuum lines are not translated (the server builds them from several
  translated pieces), error texts stay in their language (the same SQLSTATE read in several
  languages in one run counts as one kind), and reading is a few times slower than English
  because every message is matched against the translations.
- Idle time per session is not in the log, so it cannot be reported.
- The experimental cloud providers have not been confirmed against the real services.

## Development

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test
```

Tests that need Docker (real PostgreSQL 14 to 18) are ignored by default:

```bash
cargo run -p synth -- generate --pg 17     # a generated test server and its logs, in data/
cargo test -- --ignored
```

`scripts/coverage.sh` measures line coverage with `cargo-llvm-cov`. See
[CONTRIBUTING.md](https://github.com/vtrbraga/pgloupe/blob/main/CONTRIBUTING.md) for the layout and the rules,
[SECURITY.md](https://github.com/vtrbraga/pgloupe/blob/main/SECURITY.md) to report a vulnerability, and the
[docs](https://vtrbraga.github.io/pgloupe/) for guides and the command reference.

## License

Licensed under either of [MIT](https://github.com/vtrbraga/pgloupe/blob/main/LICENSE-MIT) or [Apache-2.0](https://github.com/vtrbraga/pgloupe/blob/main/LICENSE-APACHE), at your option.
