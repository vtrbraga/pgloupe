# Keep logs and compare periods

A store is one SQLite file that holds the per-hour numbers from your logs. Ingest once, then report on any window without reading the logs again.

```bash
pgloupe logs ingest --store pg.db /var/log/postgresql/*.log
pgloupe report --store pg.db --since 2026-10-14T09:00Z --until 2026-10-14T12:00Z --output html --out morning.html
```

Re-running `ingest` on the same files says "unchanged"; a file that grew is replaced; renames and compression are recognised (`--force` reads a file again anyway). `ingest` warns when two files cover the same time range, since a partial copy would count its events twice.

`report --store` writes every format and can add snapshots with `--before` and `--after`. `logs report --store` is the log-only version, text or JSON. Neither reads the log files again.

## Filters are applied while ingesting

Filters (`--database`, `--user`, `--application`, `--host`, `--between`, `--exclude-hours`, `--weekdays`, `--include-query`, and so on) and `--anonymize` shape what goes into the store, not what comes out. A store remembers the filter it was built with and refuses to mix logs ingested with a different one. `report --store` rejects filter flags and tells you to filter at ingest.

## Compare two periods

```bash
pgloupe report --store pg.db --since 2026-10-14T11:00Z --until 2026-10-14T12:00Z --baseline previous
```

`--baseline previous` compares the window with the one of the same whole-hour length right before it. For another baseline give `--baseline-since` and `--baseline-until`; they may not overlap the period. The report adds statements that are new, gone, slower or faster, and the change in errors, locks and temp files.

## Warnings in a stored report

A store keeps the warnings about each file's prefix, severity labels and time zone from the moment it was ingested, and shows them in every report. It does not keep which lines were skipped. A stored report's "not seen" list can therefore name checkpoints even when checkpoint lines were skipped, which `logs analyze` and `report` on the log files themselves tell apart. If the report says checkpoints were not seen but the ingest warned that lines did not fit the prefix, fix the prefix first and ingest again with `--force`.
