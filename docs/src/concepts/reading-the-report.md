# Reading the report

| Section | What is in it |
|---|---|
| Overview | Health and load figures: errors, lock waits, deadlocks, temp files, rollbacks, abandoned sessions; running, idle-in-transaction and connected sessions and cache hit from `pg_stat_database`. Then the actions, the top five statements and the statement kinds. A figure whose setting is off says "not logged" instead of 0 |
| What to fix first | Ranked problems, each with the numbers, the advice and the statements involved |
| Statements | Per statement: calls, total, mean, p50, p95, p99, max, spread (standard deviation over the mean), errors, lock waits, temp files; parse, bind and cursor-fetch time; from the statistics, cache hit, I/O share of execution time, blocks read, rows per call, blocks read per row and WAL per call. A *Top by* selector reorders the table on any of those. *Slower* and *faster* flag a statement whose mean moved by half or more since before the window |
| Errors | By SQLSTATE and message shape, with the statement, `DETAIL` and `HINT` |
| Locks and temp files | Waits over time, who held the lock, deadlocks, temp-file spills |
| Sessions | Connections, session durations, connections per host, sessions that ended abruptly |
| Maintenance | Checkpoints (WAL between them, causes, sync time first), autovacuum per table, schema changes and temporary tables |
| Breakdowns | By database, user, application and host. Columns the log never had are left out; when the log carries no databases or users, the statistics fill in |
| Slowest | The slowest single executions, with parameters only if you ask |
| Pooler | pgBouncer only: logins, why sessions ended, mean client wait and peak transactions per second from the `stats:` lines |
| Comparison | One period against the one before it |
| Settings | Which settings limit this report, and what to change |
| Sources | What was read: the files, the statistics window and its totals (cache hit, file I/O share, planning time, WAL, per-database and per-role rollups), and the `pg_stat_database` figures |
| Data caveats | Why a number may be off |

Sessions, Locks and Maintenance share one activity chart with a common time axis. A panel whose setting is off says "not logged" rather than showing an empty chart.

## Three sources that look alike

- **Log numbers** come from statements the server wrote to the log. They exist only for statements slower than `log_min_duration_statement`, so they are a sample of the slow ones.
- **Statistics numbers** come from `pg_stat_statements` between two snapshots and count every call, but keep no per-call detail (no percentiles, no parameters).
- **Server load** comes from `pg_stat_database` between the same two snapshots: sessions, transactions, rollbacks, cache hit, temp files, deadlocks. It also says how much of the server's statement time `pg_stat_statements` accounts for; when that share is small, the statement ranking describes a small part of the load.

The Statements view shows the first two side by side and says when they disagree.

## "Data caveats" and "Settings"

A caveat is a reason a number may be off: a reset between snapshots, evicted entries, a log with no durations, a window where the statistics cover little of the load. A setting entry says which server setting caused a gap and where to change it: in the DB parameter group on RDS, in `postgresql.conf` elsewhere. Advice read from your server's real settings (`--settings`, or the file `snap` writes) is marked as such; advice inferred from the data alone is marked as inferred.

## Evictions

`pg_stat_statements` drops its least-used entries when it is full. Its `dealloc` counter counts eviction rounds, each of which removes about 5% of `pg_stat_statements.max`, not entries. The report says how many rounds happened, estimates the entries lost, names utility statements as the cause when they dominate the churn (`track_utility = off` fixes that), and does not call churn news right after a reset.
