# Compared with pgBadger

pgBadger is the established tool and covers more log formats (syslog, ssh, `auto_explain` plans, vendor wrappers). pgloupe is narrower and does these differently:

- It joins logs with `pg_stat_statements` snapshots and understands resets and evictions.
- Percentiles are per statement, not per minute.
- A `log_line_prefix` mismatch is diagnosed, not silent.
- Statements are grouped by their parse tree, not by regular expressions.
- Reports for a time window come from a store, without re-parsing.
- One static binary instead of Perl.
- It says which settings limit the report, and what to change.

On a generated 60 MB PostgreSQL 17 stderr log, one thread: pgBadger 8.7 s (about 7 MB/s), pgloupe 0.32 s. Counts agree where both measure the same thing (statements within one of each other, temp files equal). Numbers are from October 2026 and will age; measure on your own logs.
