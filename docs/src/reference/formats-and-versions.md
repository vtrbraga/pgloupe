# Supported versions and formats

| | |
|---|---|
| PostgreSQL | 14 to 18, each tested against a real server |
| Log formats | stderr (any `log_line_prefix`, inferred), csvlog, jsonlog |
| Compression | gzip, zstd, xz, lz4, bzip2, detected from the file contents |
| Statistics | CSV exports of `pg_stat_statements` and `pg_stat_statements_info`, which `snap` writes |
| Experimental | pgBouncer logs; Portuguese (Brazil), Spanish, French, German servers; clouds other than Amazon RDS |
| Platforms | Linux and macOS tested in CI; Windows experimental |

Multi-line entries, application names with spaces and Amazon RDS truncation markers are handled.

## Output

- **JSON** has a `schema_version`. While pgloupe is before `0.1.0` the schema can change between releases; the changelog says when.
- **CSV** is the statement table, with formula-looking cells quoted so a spreadsheet does not run them.
- **HTML** is one self-contained file with no network requests.
- **Stores** carry a schema version. A store from an older release is refused with a message saying how to rebuild it.
