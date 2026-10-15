# Amazon RDS

You need the `aws` CLI signed in, a database role that can read statistics, and these settings in the parameter group. `pgloupe doctor` checks all of them and says what is missing.

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

## One command

```bash
export PGLOUPE_DSN='postgres://me@my-db.abc.us-east-1.rds.amazonaws.com:5432/postgres'  # password: PGPASSWORD or ~/.pgpass
pgloupe doctor --vendor rds
pgloupe export --provider aws-rds --instance my-db --since 24h --out export/ --report report.html
```

`export` checks the server, takes a snapshot, downloads the logs and builds the report in one go.

## Good to know

- RDS logs are hourly files named `error/postgresql.log.<date>-<hour>`. pgloupe downloads them resumably and notices when a file was rewritten.
- RDS fixes `log_line_prefix`, which has no application name or SQLSTATE. The report says which columns that leaves empty.
- RDS cuts very long entries and prints a marker. pgloupe counts them and keeps the entry's context and statement.
- A heavily-used database can overflow `pg_stat_statements.max` many times over. The report counts the evicted entries and, when it can, names the cause.
