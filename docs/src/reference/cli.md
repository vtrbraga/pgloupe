# Command line

Run any command with `--help`. This page lists what exists; `--help` is always current.

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

## Flags that work on `report`, `logs analyze` and `logs ingest`

- `--database`, `--user`, `--application`, `--host` (and `--exclude-…`): exact names, globs (`pgAdmin*`) or `re:regex`.
- `--between 09:00-18:00`, `--exclude-hours 5-7`, `--weekdays mon-fri`, `--tz Europe/Paris`, `--pid`, `--exclude-message`, `--include-query`, `--exclude-query`.
- `--since`, `--until` and `--baseline previous` on a store report.
- `--anonymize` (with `--anonymize-key` or `PGLOUPE_ANONYMIZE_KEY`).
- `--top N|all`, `--by total|calls|p95|max`, `--slowest N`, `--params`, `--bucket 30s|1m|1h`.
- `--format`, `--prefix`, `--log-tz`, `--lc-messages`, `--messages-file`, `--threads`, `--max-fingerprints`.

## Flags that work on `report` only

- `--before`, `--after`, `--before-info`, `--after-info`, `--before-db`, `--after-db`: the two snapshots and the `pg_stat_statements_info` and `pg_stat_database` files taken with them. The `_db` files are found beside the snapshots when `snap` wrote them.
- `--settings`: `pg_settings` as CSV, picked up from beside `--after` when `snap` wrote it; `--vendor rds` words the advice for RDS.
- `--display-tz`: show times in an IANA zone in the HTML report (JSON and text stay UTC).
- `--output text,json,html,csv` and `--out`. `logs analyze`, `logs report` and `stats diff` write text or JSON only.

## Exit codes

## Exit codes

| Code | Meaning |
|---|---|
| 0 | success |
| 1 | the work failed: unreadable input, nothing decoded, a server or vendor error |
| 2 | the command line is wrong |

## Environment variables

| Variable | Used for |
|---|---|
| `PGLOUPE_DSN` | connection string for `snap`, `doctor`, `export`, `watch` and `logs fetch --provider sql` |
| `PGPASSWORD`, `PGSSLMODE`, `PGSSLROOTCERT`, `PGPASSFILE` | the usual libpq meanings |
| `PGLOUPE_ANONYMIZE_KEY` | the anonymizing key |
