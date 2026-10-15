# Follow a live server

`watch` loops `snap`, fetch and report, and serves the live dashboard on a local address.

```bash
pgloupe watch --conn postgres://me@db.example.com/postgres \
  --provider aws-rds --instance my-db --interval 5m --window 24h
```

It prints a local URL. The page keeps your view, search, selected row and scroll position when it refreshes, and says when the `watch` process behind it restarted.

| Flag | Default | What it does |
|---|---|---|
| `--interval` | `5m` | time between updates |
| `--window` | `24h` | how far back the report looks |
| `--listen` | `127.0.0.1:8080` | where the dashboard is served; loopback only unless `--allow-remote`, since the page holds statement text |
| `--state-dir` | `pgloupe-watch` | where snapshots, fetched logs and the store are kept |
| `--keep-snapshots` | `288` | snapshots to keep (24 hours at 5 minutes) |
| `--out` | | also write the HTML here after every update |
| `--once` | | run one cycle and exit instead of looping and serving |

## The pieces

| Command | What it does |
|---|---|
| `doctor` | checks the server and the role and says how to fix what is missing; `--output json` for scripts |
| `snap` | takes a `pg_stat_statements` snapshot, with `pg_stat_database` and the settings; read-only |
| `logs fetch` | downloads log files from RDS (and, experimentally, other providers), resumable |
| `export` | `doctor`, `snap`, `logs fetch` and `report` in one command |
| `watch` | repeats all of that and serves the result |

## Connection

Pass `--conn` or set `PGLOUPE_DSN`. Passwords come from `PGPASSWORD` (or the variable named by `--password-env`) or `~/.pgpass`; keep them out of the command line. `PGSSLMODE` and `PGSSLROOTCERT` are honoured; `verify-ca` and `verify-full` check the server certificate, and `--sslrootcert` gives the CA to trust (for RDS, the regional bundle from AWS). A Unix socket skips TLS.
