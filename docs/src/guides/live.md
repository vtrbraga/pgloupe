# Follow a live server

`watch` loops `snap`, fetch and report, and serves the live dashboard on a local address. Nothing leaves your machine.

```bash
pgloupe watch --conn postgres://me@db.example.com/postgres \
  --provider aws-rds --instance my-db --interval 5m --window 24h
```

It prints a local URL. The page keeps your view, search, selected row and scroll position when it refreshes, and says when the server behind it restarted.

## The pieces

| Command | What it does |
|---|---|
| `doctor` | checks the server and the role and says how to fix what is missing |
| `snap` | takes a `pg_stat_statements` snapshot (and the settings); read-only |
| `logs fetch` | downloads log files from RDS (and, experimentally, other providers), resumable |
| `export` | `doctor`, `snap`, `logs fetch` and `report` in one command |
| `watch` | repeats all of that and serves the result |

## Connection

Pass `--conn` or set `PGLOUPE_DSN`. Passwords come from `PGPASSWORD` or `~/.pgpass`, never from the command line. `PGSSLMODE` and `PGSSLROOTCERT` are honoured; `verify-ca` and `verify-full` check the server certificate. A Unix socket skips TLS.
