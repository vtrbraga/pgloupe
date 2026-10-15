# How a report is made

pgloupe reads inputs and builds one report in stages. Each stage hands the next only what it needs.

```text
logs ──► decoder ──► assembler ──► classifier ──► accumulators ──► report
                                                         ▲
snapshots ──► delta (reset and eviction aware) ───────────┘
```

1. **Decoder.** Reads stderr, csvlog or jsonlog, plain or compressed. For stderr it infers the `log_line_prefix` from a sample and prints the one it chose; if no candidate matches nearly every line it says so and stops.
2. **Assembler.** Joins continuation lines to their entry, per backend process, and attaches `DETAIL`, `HINT`, `CONTEXT` and `STATEMENT` lines to the entry they belong to.
3. **Classifier.** Turns each entry into an event: a statement with or without a duration, a lock wait, a temp file, a checkpoint, an autovacuum run, a connection, an error, and so on.
4. **Correlation.** Connects a duration to the statement it belongs to when the server logged them apart (cursors, prepared statements, parse/bind/execute), and names the statement that held a lock another one waited for.
5. **Accumulators.** Count and keep, per statement fingerprint, calls, time, percentiles, errors, lock waits and temp files. A fingerprint comes from the statement's parse tree (`libpg_query`), so `WHERE id = 1` and `WHERE id = 2` are one statement.
6. **Join.** If you gave snapshots, `pg_stat_statements` counts for the window are matched to the log's statements by fingerprint. pgloupe notices resets, evictions and windows that do not overlap, and says so instead of showing a wrong number.
7. **Findings and advice.** Rules turn the numbers into *What to fix first* and into *Settings* advice, each with the evidence behind it.

## What it does not do

It reads files you give it or fetch with its own commands; it is not a monitoring system and it never changes your server. The only statements it runs are read-only queries against `pg_stat_statements`, `pg_settings` and a few catalog views.
