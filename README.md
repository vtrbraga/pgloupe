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

Summarize one or more logs (csvlog, jsonlog or stderr; plain, gzip, zstd, xz, lz4 or bzip2): statements grouped by fingerprint with p50/p95/p99, query types, duration histograms, cancelled queries, the slowest executions, errors, locks and deadlocks, temp files, connections and sessions, checkpoints, autovacuum, breakdowns by database, user, application and host, and a timeline.

```bash
cargo run -p pgloupe -- logs analyze postgresql.log.gz
cargo run -p pgloupe -- logs analyze a.csv b.csv --by p95 --top 30 --output json > report.json
cargo run -p pgloupe -- logs analyze server.log --prefix '%t:%r:%u@%d:[%p]:' --threads 4
```

Filters apply while the files are read: `--database`, `--user`, `--application`, `--host` (repeat or comma-separate), their `--exclude-…` forms, and `--include-query` / `--exclude-query` (a regular expression tested on statement text only; errors, lock waits and bare `duration:` lines are never judged by it). A record without the attribute fails an include list and passes an exclude list; connection records carry no database or user yet, so a database or user filter drops them. The report says how many records were dropped. Filters are not available with `--store`.

`--slowest N` (default 20) lists the slowest single executions. Bind parameters are kept only with `--params`, since they can hold personal data; without it the list shows normalized statement text.

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
- The store keeps the top 1,000 values per breakdown dimension per hour, and the slowest executions and bind parameters that its ingest asked for (`--slowest`, `--params`). A store written by an older pgloupe (schema 1 or 2) is refused: delete it and ingest again.
- `--force` re-ingests files already in the store, for example after fixing `--prefix`. A file that decodes to nothing is reported as `failed` and not stored.
- One ingest runs at a time per store (a second one waits 5 s, then reports the store as locked). If a file's time range largely overlaps an earlier input that it does not extend, ingest prints a warning: an older partial copy of a stored log is counted twice.
- Keep the store on the Linux filesystem; SQLite's WAL mode is unreliable on `/mnt/c`.
- With `--max-fingerprints` below the number of distinct statements in an hour, that hour folds the rest into the overflow row, so `report` can differ from `logs analyze` for that run.

## Report

`pgloupe report` makes one report from logs and, optionally, two `pg_stat_statements` snapshots, and joins them per statement: what the log saw (percentiles, errors, lock waits, temp files) next to what the statistics counted (calls, time, rows, buffers, WAL).

```bash
cargo run -p pgloupe -- report postgresql.log --output html --out report.html
cargo run -p pgloupe -- report postgresql.log.gz --before pgss_1.csv --after pgss_2.csv \
    --before-info pgss_info_1.csv --after-info pgss_info_2.csv --output html --out report.html
cargo run -p pgloupe -- report --store logs.db --before pgss_1.csv --after pgss_2.csv --output json
cargo run -p pgloupe -- report --before pgss_1.csv --after pgss_2.csv          # snapshots only
```

- The report opens with **what to fix first**: deadlocks, statements cancelled by `statement_timeout`/`lock_timeout`, one statement dominating execution time, lock waits, temp-file spills and errors (with advice per SQLSTATE), each with its numbers, what to do about it, a line from the log and the statements involved. What used to be called findings are now **data caveats**: how far to trust the numbers. Errors carry their statements, DETAIL, HINT and process id; each statement keeps its slowest run (10 ms or more) and who ran it.
- Formats: `text` (default), `json` (`schema_version: 1`, `kind: "pgloupe.report"`) and `html`: an interactive dashboard in one file with no network requests. It has an overview with a time-range selection, a searchable and sortable statement table with a detail drawer, and views for errors, locks, sessions, maintenance, breakdowns and the slowest executions; its state lives in the URL hash (`/` searches, `j`/`k` move, `1`-`9` switch views). The server-rendered tables stay in the file for printing and for browsers without JavaScript. Log text is only ever inserted as text.
- `--database`, `--user`, `--application`, `--host`, `--include-query`, `--exclude-query`, `--slowest` and `--params` work on log `FILES` as in `logs analyze`. Filtered logs are joined with unfiltered statistics, so coverage then reflects the filter.
- Statements are joined by fingerprint. Several statistics entries with one fingerprint (users, databases, partitions with different `queryid`s) are summed; `track = all` entries for statements run inside functions are kept apart as `nested_calls`.
- `coverage` is log calls with a duration divided by the statistics' calls: below 100% when `log_min_duration_statement` hides fast calls, above when the log window is wider than the snapshot window.
- With `--store`, the log window is the snapshot interval widened to whole hours unless `--since/--until` are given.
- The `findings` block explains odd results: resets and evictions, unreadable query text (grant `pg_read_all_stats`), windows that do not match, statements missing from one side, partitions rolled up.

## Collect from a live server

`snap`, `doctor`, `logs fetch`, `export` and `watch` read a running server. They never write to it: the session is read-only, `pg_stat_statements_reset()` is never called, and nothing is installed.

```bash
export PGLOUPE_DSN='postgres://pgloupe@mydb.abc.eu-west-1.rds.amazonaws.com:5432/postgres'   # password: PGPASSWORD or ~/.pgpass
pgloupe doctor --vendor rds                      # what the server or role lacks, and how to fix it
pgloupe snap --out snaps/                        # pgss_<time>.csv + pgss_info_<time>.csv
pgloupe logs fetch --provider aws-rds --instance mydb --since 24h --out logs/
pgloupe export --provider aws-rds --instance mydb --since 24h --out export/ --report report.html
pgloupe watch --provider aws-rds --instance mydb --interval 5m --window 24h    # prints http://127.0.0.1:8080/
```

- **Connection:** `--conn` or `PGLOUPE_DSN`. TLS is on by default (`prefer`); `sslmode=verify-full` or `--sslrootcert FILE` checks the certificate and host name (RDS: the regional bundle from AWS). Passwords are never printed.
- **`watch`** loops: snapshot, fetch new log bytes, ingest into a SQLite store in `--state-dir`, rebuild the report for the last `--window`, and serve it with `tiny_http` on loopback (`--listen`, `--allow-remote` to expose it; the page contains statement text). The dashboard polls `/status` and swaps in each new report keeping its view; a failed cycle keeps the last good report, shows a banner and retries with backoff. Routes: `/`, `/report.json`, `/status`, `/healthz`. `--once` runs one cycle and exits.
- **Providers** read logs through the vendor's CLI (argument lists, no shell, your own credentials). Use `--bin` to point at a CLI that is not on `PATH`.

| Provider | Status | Needs |
|---|---|---|
| `aws-rds` (RDS, Aurora PostgreSQL) | supported | `aws` CLI; IAM `rds:DescribeDBLogFiles`, `rds:DownloadDBLogFilePortion` |
| `sql` (self-managed) | experimental | role allowed `pg_ls_logdir()` and `pg_read_binary_file()` (`pg_read_server_files`) |
| `aws-cloudwatch` | experimental | `aws` CLI; log export to CloudWatch on; `logs:FilterLogEvents` |
| `azure-flexible` | experimental | `az` CLI; server log download enabled |
| `gcp-cloudsql` | experimental | `gcloud`; `logging.logEntries.list` |
| `aiven` | experimental | `avn`, logged in |
| `heroku` | experimental | `heroku` CLI; recent lines only |

Experimental connectors are tested against a fake CLI replaying payloads written from the vendor's documentation, not against the real services, and print a warning on every use.

**What the server needs** (`doctor` checks all of this):

- Database role: `GRANT pg_monitor TO pgloupe;` (or `pg_read_all_stats` and `pg_read_all_settings`) so query text and settings are visible; `CREATE EXTENSION pg_stat_statements;` in the database.
- Parameter group (RDS) or `postgresql.conf`: `shared_preload_libraries` includes `pg_stat_statements` (RDS default; a reboot applies it), `log_min_duration_statement` (for example `250`), `log_lock_waits = on`, `log_temp_files = 0`, `log_checkpoints = on`, `log_autovacuum_min_duration` (for example `1000`), `log_connections` and `log_disconnections = on`, `track_io_timing = on`, `compute_query_id = auto`. RDS fixes `log_line_prefix`, so application name and SQLSTATE columns are empty there.

## Benchmark input

`cargo run --release --example gen_log -- OUT RECORDS HOURS FINGERPRINTS [SEED]` writes a deterministic jsonlog with many hours and many distinct statements (per tenant and partition), for timing `logs analyze`, `logs ingest` and `logs report` on something larger and wider than the synth corpus.

## Postgres versions

Logs and snapshots are read from Postgres 14 to 18; the synthetic corpus runs on every one of them (`cargo run -p synth -- generate --pg 14`, then `scripts/corpus-matrix.sh`).

| Version | What to know |
|---|---|
| 14 | No `jsonlog` (it arrived in 15): use stderr or csvlog. `CREATE DATABASE` forces checkpoints. `pg_stat_statements_info` and `toplevel` exist. |
| 15, 16 | `jsonlog` available. No `stats_since` column. |
| 17 | `stats_since` and `minmax_stats_since`; the server logs a `connection authenticated` line. |
| 18 | Newer `pg_stat_statements` columns are accepted; the logs read like 17's. |

Snapshots from different major versions are never compared. `pg_stat_statements` CSVs from Postgres 13 are read (`total_time` is explained) but not tested.

## Synthetic corpus

`synth` generates Postgres logs and `pg_stat_statements` snapshots from a seeded workload on the Pagila sample database. Requires Docker.

```bash
cargo run -p synth -- generate            # data/synth/17-42/
cargo run -p synth -- generate --pg 16 --seed 7 --only lock_wait,errors --force   # --pg 14|15|16|17|18
```

Output is gitignored. `manifest.json` lists the expected log events per scenario.

Docker-backed tests: `cargo test -p synth -- --ignored`.
