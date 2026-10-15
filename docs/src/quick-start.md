# Quick start

## A report from a log file

```bash
pgloupe report postgresql.log --output html --out report.html
```

pgloupe finds the `log_line_prefix` by itself and prints what it chose. If it cannot, it says why instead of printing an empty report. Open `report.html` in a browser; it is one self-contained file.

Other formats come from the same run: `--output text,json,html,csv`. With more than one format, `--out` is a path prefix (`--out report` writes `report.html` and `report.json`).

## Add two statistics snapshots

`pg_stat_statements` counts every call, including the ones the log never shows. Take a snapshot now and another later; the report is about what happened between them.

```bash
pgloupe snap --conn postgres://me@db.example.com/postgres --out snaps/
# ... wait the time you want to look at, then take another ...
pgloupe snap --conn postgres://me@db.example.com/postgres --out snaps/

pgloupe report postgresql.log \
  --before snaps/pgss_<older>.csv --after snaps/pgss_<newer>.csv \
  --before-info snaps/pgss_info_<older>.csv --after-info snaps/pgss_info_<newer>.csv \
  --output html --out report.html
```

`snap` is read-only and never resets anything. It also writes the server's settings next to each snapshot, so the report can say which settings limit it.

## Statistics only

```bash
pgloupe report --before old.csv --after new.csv
```

## Something looks wrong

- **"0 records matched" or an empty report:** the report names the prefix it tried. Give the real one with `--prefix`, for example `--prefix '%m [%p] %q%u@%d/%a '`.
- **No statements with durations:** the log has none because `log_min_duration_statement` is off. The report's *Settings* view says what to set.
- **Times look shifted:** log lines with a time-zone name the parser does not know need `--log-tz`, for example `--log-tz +02`.
- `pgloupe doctor` checks your server and role and says what is missing.

Next: [Amazon RDS](guides/rds.md), or [keep a live dashboard](guides/live.md).
