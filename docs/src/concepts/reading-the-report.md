# Reading the report

| Section | What is in it |
|---|---|
| What to fix first | Ranked problems, each with the numbers, the advice and the statements involved |
| Statements | Per statement: calls, total, mean, p50, p95, p99, max, errors, lock waits, temp files; parse, bind and cursor-fetch time; the log next to `pg_stat_statements` |
| Errors | By SQLSTATE and message shape, with the statement, `DETAIL` and `HINT` |
| Locks and temp files | Waits over time, who held the lock, deadlocks, temp-file spills |
| Sessions | Connections, session durations, connections per host, sessions that ended abruptly |
| Maintenance | Checkpoints (WAL between them, causes), autovacuum per table, schema changes and temporary tables |
| Breakdowns | By database, user, application and host |
| Slowest | The slowest single executions, with parameters only if you ask |
| Comparison | One period against the one before it |
| Settings | Which settings limit this report, and what to change |
| Data caveats | Why a number may be off |

## Two numbers that look alike

- **Log numbers** come from statements the server wrote to the log. They exist only for statements slower than `log_min_duration_statement`, so they are a sample of the slow ones.
- **Statistics numbers** come from `pg_stat_statements` between two snapshots and count every call, but keep no per-call detail (no percentiles, no parameters).

The Statements view shows both side by side and says when they disagree.

## "Data caveats" and "Settings"

A caveat is a reason a number may be off (a reset between snapshots, evicted entries, a log with no durations). A setting entry says which server setting caused a gap and where to change it: in the DB parameter group on RDS, in `postgresql.conf` elsewhere. Advice read from your server's real settings is marked as such; advice inferred from the data alone is marked as inferred.
