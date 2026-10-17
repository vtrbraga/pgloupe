# Supported versions and formats

| | |
|---|---|
| PostgreSQL | 14 to 18, each tested against a real server |
| Log formats | stderr (any `log_line_prefix`, inferred), csvlog, jsonlog |
| Compression | gzip, zstd, xz, lz4, bzip2, detected from the file contents |
| Statistics | CSV exports of `pg_stat_statements`, `pg_stat_statements_info`, `pg_stat_database` and `pg_settings`, which `snap` writes beside each other |
| Experimental | pgBouncer logs; Portuguese (Brazil), Spanish, French, German servers; clouds other than Amazon RDS |
| Platforms | Linux and macOS tested in CI; Windows experimental |

Multi-line entries, application names with spaces, IPv6 clients in `%r` and Amazon RDS truncation markers are handled. A `log_line_prefix` that puts a colon right after `%h` (for example `%h:%u`) reads an IPv6 client as the text before its first colon, because the address and the next field cannot be told apart; put a space after `%h`.

## Not read

- Syslog output and `auto_explain` plans.
- Logs on a remote machine, other than through `logs fetch` (vendor CLIs, or `--provider sql` for a server that exposes its log directory).
- A log line, an assembled entry or a csvlog record over 16 MiB: it is skipped and counted in the diagnostics, instead of being read whole into memory. For csvlog, the usual cause is an unterminated quote, and the file stops there.

## Output

- **JSON** has a `schema_version`. While pgloupe is before `0.1.0` the schema can change between releases; the changelog says when.
- **CSV** is the statement table, with formula-looking cells quoted so a spreadsheet does not run them.
- **HTML** is one self-contained file with no network requests.
- **Stores** carry a schema version. A store from an older release is refused with a message saying to delete it and ingest again.
