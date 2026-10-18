# pgloupe

A close look at your Postgres logs and query stats, in one offline binary.

Point it at a server's log files and, if you have them, two `pg_stat_statements` snapshots. It tells you **what to fix first**, then shows the numbers behind it: slow statements with real percentiles, errors, lock waits and who blocked whom, temp files, checkpoints, autovacuum, connections. It also says which server settings limit the report and what to change.

![The overview of a pgloupe report](https://raw.githubusercontent.com/vtrbraga/pgloupe/main/assets/dashboard-overview.png)

<sub>A report over a generated test server, not real data.</sub>

## Why this one

- **One file in, one file out.** No agent, no extension, no database to host. The HTML report is a single file that makes no network requests; JSON, text and CSV come from the same run.
- **Logs and statistics together.** The log shows what happened (errors, locks, slow runs); `pg_stat_statements` counts everything the log never saw, and `pg_stat_database` says how loaded the server was. pgloupe joins them per statement and says when they disagree.
- **One row per statement.** Statements are grouped by their parse tree, so the same query with different literals is one row, and percentiles are per statement.
- **Works on managed Postgres.** It only needs `pg_stat_statements`, which Amazon RDS, Cloud SQL, Azure and Aiven already have. It can fetch RDS logs itself.
- **Tells you why a number is odd.** Resets, evictions, windows that do not match, unreadable query text, logs without durations: each becomes a plain sentence, not a silent zero.
- **Fast.** On a 10.7 GB set of 40 generated PostgreSQL 17 logs it read about 200 MB/s on one thread using about 46 MiB of memory, and about 1.2 GB/s on 16 threads (512 MiB). Memory depends on the number of distinct statements, not on log size. Figures vary by hardware.
- **Safe to share.** `--anonymize` rewrites names and literals before anything is counted.

![The Statements view of a pgloupe report](https://raw.githubusercontent.com/vtrbraga/pgloupe/main/assets/dashboard-statements.png)

<sub>The Statements view over the same generated test server.</sub>

## Try it

```bash
pgloupe report postgresql.log --output html --out report.html
```

New here? [Install](install.md) it, then follow the [quick start](quick-start.md). On Amazon RDS, go straight to the [RDS guide](guides/rds.md).

pgloupe never phones home: it makes no network connection unless you run a command that talks to your server, and it does not check for updates by itself.
