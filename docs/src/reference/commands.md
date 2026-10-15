# Command reference

Generated from the program; `pgloupe <command> --help` shows the same text.

## `pgloupe`

```text
A close look at your Postgres logs and query stats.

Usage: pgloupe <COMMAND>

Commands:
  stats   Work with `pg_stat_statements` snapshots
  logs    Work with Postgres server logs
  report  One report from logs and, optionally, two `pg_stat_statements` snapshots: HTML, JSON or text, with the two joined per statement
  snap    Take a `pg_stat_statements` snapshot from a live server (read-only; never resets anything)
  doctor  Check what the server and the role lack for a useful report, and how to fix it
  export  Check the server, take a snapshot and fetch logs into one directory; optionally the report
  watch   Keep a report fresh from a live server and serve it locally
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help

  -V, --version
          Print version

Exit codes: 0 success; 1 the work failed (unreadable input, nothing decoded, a server or vendor error); 2 the command line is wrong.
Postgres 14 to 18 are supported; logs may be stderr, csvlog or jsonlog, plain or gzip, zstd, xz, lz4 or bzip2.
```

## `pgloupe stats`

```text
Work with `pg_stat_statements` snapshots

Usage: pgloupe stats <COMMAND>

Commands:
  diff  Compare two snapshot CSV exports and rank the queries that changed
  help  Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

## `pgloupe stats diff`

```text
Compare two snapshot CSV exports and rank the queries that changed

Usage: pgloupe stats diff [OPTIONS] <BEFORE> <AFTER>

Arguments:
  <BEFORE>
          Older snapshot: CSV export of `pg_stat_statements`

  <AFTER>
          Newer snapshot

Options:
      --before-info <BEFORE_INFO>
          `pg_stat_statements_info` CSV taken with the older snapshot

      --after-info <AFTER_INFO>
          `pg_stat_statements_info` CSV taken with the newer snapshot

      --by <BY>
          Metric that orders the table

          Possible values:
          - total: Δ total execution time
          - calls: Δ calls
          - mean:  Mean execution time inside the window
          
          [default: total]

      --top <TOP>
          Rows to show (`all` for every statement)
          
          [default: 20]

      --min-calls <MIN_CALLS>
          With `--by mean`, skip queries with fewer calls than this in the window
          
          [default: 1]

      --wide
          Show full query text in the table

      --format <FORMAT>
          Output format
          
          [default: text]
          [possible values: text, json]

  -h, --help
          Print help (see a summary with '-h')
```

## `pgloupe logs`

```text
Work with Postgres server logs

Usage: pgloupe logs <COMMAND>

Commands:
  dump     Decode a log (csvlog, jsonlog or stderr; plain or gzip) and print each record as JSON
  analyze  Summarize one or more logs: statements with percentiles, errors, locks, temp files, connections, checkpoints and autovacuum
  ingest   Add logs to a store; files already ingested are skipped and grown ones replace their old version
  report   Report on a time window of a store
  fetch    Download a time range of log files from a managed service or server (resumable)
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

## `pgloupe logs dump`

```text
Decode a log (csvlog, jsonlog or stderr; plain or gzip) and print each record as JSON

Usage: pgloupe logs dump [OPTIONS] <FILE>

Arguments:
  <FILE>
          Log file; compression is detected automatically

Options:
      --format <FORMAT>
          Log format; `auto` looks at the first line

          Possible values:
          - auto
          - csv
          - json
          - stderr
          - pgbouncer: The pgBouncer log (experimental)
          
          [default: auto]

      --prefix <PREFIX>
          The server's `log_line_prefix`; required for stderr logs

      --log-tz <LOG_TZ>
          Offset for timezone abbreviations the log does not spell out, e.g. `-03` or `+05:30`

      --limit <LIMIT>
          Stop after this many records

  -h, --help
          Print help (see a summary with '-h')
```

## `pgloupe logs analyze`

```text
Summarize one or more logs: statements with percentiles, errors, locks, temp files, connections, checkpoints and autovacuum

Usage: pgloupe logs analyze [OPTIONS] <FILES>...

Arguments:
  <FILES>...
          Log files (csvlog, jsonlog or stderr; compression is detected automatically)

Options:
      --format <FORMAT>
          Log format; `auto` looks at each file's first line

          Possible values:
          - auto
          - csv
          - json
          - stderr
          - pgbouncer: The pgBouncer log (experimental)
          
          [default: auto]

      --prefix <PREFIX>
          The server's `log_line_prefix`; inferred when omitted

      --log-tz <LOG_TZ>
          Offset for timezone abbreviations the log does not spell out, e.g. `-03` or `+05:30`

      --by <BY>
          Metric that orders the statement table

          Possible values:
          - total: Total execution time
          - calls: Number of calls
          - p95:   95th percentile of the duration
          - max:   Slowest single call
          
          [default: total]

      --top <TOP>
          Statements to list (`all` for every statement)
          
          [default: 20]

      --bucket <BUCKET>
          Timeline resolution: `30s`, `1m`, `1h`
          
          [default: 1m]

      --max-fingerprints <MAX_FINGERPRINTS>
          Distinct statements kept before the rest fold into one overflow row
          
          [default: 50000]

      --threads <THREADS>
          Worker threads (default: all cores)

      --output <OUTPUT>
          Report format
          
          [default: text]
          [possible values: text, json]

      --slowest <SLOWEST>
          Slowest single executions to keep and list (default 20)

      --params
          Keep the bind parameters of those executions; they can hold personal data

      --database <DATABASES>
          Only records of these databases (repeat or comma-separate)

      --user <USERS>
          Only records of these users

      --application <APPLICATIONS>
          Only records of these applications (`application_name`)

      --host <HOSTS>
          Only records from these client hosts

      --exclude-database <EXCLUDE_DATABASES>
          Drop records of these databases

      --exclude-user <EXCLUDE_USERS>
          Drop records of these users

      --exclude-application <EXCLUDE_APPLICATIONS>
          Drop records of these applications

      --exclude-host <EXCLUDE_HOSTS>
          Drop records from these client hosts

      --include-query <INCLUDE_QUERY>
          Only records whose message or statement matches this regular expression

      --exclude-query <EXCLUDE_QUERY>
          Drop records whose message or statement matches this regular expression

      --pid <PIDS>
          Only records of these backend process ids

      --exclude-message <EXCLUDE_MESSAGE>
          Drop records whose message matches this regular expression (noise such as health checks)

      --between <BETWEEN>
          Only records inside this time of day, as `HH:MM-HH:MM` (wraps past midnight)

      --exclude-hours <EXCLUDE_HOURS>
          Drop records in these hours of the day, as `5-7` (hours 5 to 7 inclusive); repeatable

      --weekdays <WEEKDAYS>
          Only records on these days: `mon-fri`, `sat,sun`

      --tz <TZ>
          Zone for `--between`, `--exclude-hours` and `--weekdays`: an IANA name or `UTC` (default)

      --anonymize
          Make the report safe to share: users, databases, applications and hosts become stable pseudonyms, and literals, emails and addresses are removed from every text

      --anonymize-key <ANONYMIZE_KEY>
          Secret that decides the pseudonyms (default: none, so common names are guessable)
          
          [env: PGLOUPE_ANONYMIZE_KEY]

      --lc-messages <LC_MESSAGES>
          The language of the server's messages (`lc_messages`): `auto` (default) looks at the log, `en` reads English, or one of `pt_BR`, `es`, `fr`, `de`
          
          [default: auto]

      --messages-file <MESSAGES_FILE>
          A translation table for another language: lines of `locale<TAB>english<TAB>translated`, as in the built-in one

  -h, --help
          Print help (see a summary with '-h')
```

## `pgloupe logs ingest`

```text
Add logs to a store; files already ingested are skipped and grown ones replace their old version

Usage: pgloupe logs ingest [OPTIONS] --store <STORE> <FILES>...

Arguments:
  <FILES>...
          Log files (csvlog, jsonlog or stderr; compression is detected automatically)

Options:
      --store <STORE>
          The store file; created when missing

      --format <FORMAT>
          Log format; `auto` looks at each file's first line

          Possible values:
          - auto
          - csv
          - json
          - stderr
          - pgbouncer: The pgBouncer log (experimental)
          
          [default: auto]

      --prefix <PREFIX>
          The server's `log_line_prefix`; inferred when omitted

      --log-tz <LOG_TZ>
          Offset for timezone abbreviations the log does not spell out, e.g. `-03` or `+05:30`

      --max-fingerprints <MAX_FINGERPRINTS>
          Distinct statements kept per hour before the rest fold into one overflow row
          
          [default: 50000]

      --threads <THREADS>
          Worker threads (default: all cores)

      --force
          Read and store files even when their content is already in the store

      --slowest <SLOWEST>
          Slowest single executions to keep and list (default 20)

      --params
          Keep the bind parameters of those executions; they can hold personal data

      --database <DATABASES>
          Only records of these databases (repeat or comma-separate)

      --user <USERS>
          Only records of these users

      --application <APPLICATIONS>
          Only records of these applications (`application_name`)

      --host <HOSTS>
          Only records from these client hosts

      --exclude-database <EXCLUDE_DATABASES>
          Drop records of these databases

      --exclude-user <EXCLUDE_USERS>
          Drop records of these users

      --exclude-application <EXCLUDE_APPLICATIONS>
          Drop records of these applications

      --exclude-host <EXCLUDE_HOSTS>
          Drop records from these client hosts

      --include-query <INCLUDE_QUERY>
          Only records whose message or statement matches this regular expression

      --exclude-query <EXCLUDE_QUERY>
          Drop records whose message or statement matches this regular expression

      --pid <PIDS>
          Only records of these backend process ids

      --exclude-message <EXCLUDE_MESSAGE>
          Drop records whose message matches this regular expression (noise such as health checks)

      --between <BETWEEN>
          Only records inside this time of day, as `HH:MM-HH:MM` (wraps past midnight)

      --exclude-hours <EXCLUDE_HOURS>
          Drop records in these hours of the day, as `5-7` (hours 5 to 7 inclusive); repeatable

      --weekdays <WEEKDAYS>
          Only records on these days: `mon-fri`, `sat,sun`

      --tz <TZ>
          Zone for `--between`, `--exclude-hours` and `--weekdays`: an IANA name or `UTC` (default)

      --anonymize
          Make the report safe to share: users, databases, applications and hosts become stable pseudonyms, and literals, emails and addresses are removed from every text

      --anonymize-key <ANONYMIZE_KEY>
          Secret that decides the pseudonyms (default: none, so common names are guessable)
          
          [env: PGLOUPE_ANONYMIZE_KEY]

      --lc-messages <LC_MESSAGES>
          The language of the server's messages (`lc_messages`): `auto` (default) looks at the log, `en` reads English, or one of `pt_BR`, `es`, `fr`, `de`
          
          [default: auto]

      --messages-file <MESSAGES_FILE>
          A translation table for another language: lines of `locale<TAB>english<TAB>translated`, as in the built-in one

  -h, --help
          Print help (see a summary with '-h')
```

## `pgloupe logs report`

```text
Report on a time window of a store

Usage: pgloupe logs report [OPTIONS] --store <STORE>

Options:
      --store <STORE>
          The store file written by `logs ingest`

      --slowest <SLOWEST>
          Slowest executions to list, at most what the ingest kept (20 unless it was told more)

      --since <SINCE>
          Start of the window (RFC 3339 or `YYYY-MM-DD`, UTC); rounded down to the hour

      --until <UNTIL>
          End of the window; rounded up to the hour

      --by <BY>
          Metric that orders the statement table

          Possible values:
          - total: Total execution time
          - calls: Number of calls
          - p95:   95th percentile of the duration
          - max:   Slowest single call
          
          [default: total]

      --top <TOP>
          Statements to list (`all` for every statement)
          
          [default: 20]

      --bucket <BUCKET>
          Timeline resolution: a multiple of one minute
          
          [default: 1m]

      --output <OUTPUT>
          Report format
          
          [default: text]
          [possible values: text, json]

  -h, --help
          Print help (see a summary with '-h')
```

## `pgloupe logs fetch`

```text
Download a time range of log files from a managed service or server (resumable)

Usage: pgloupe logs fetch [OPTIONS] --provider <PROVIDER> --out <OUT>

Options:
      --provider <PROVIDER>
          Log source

          Possible values:
          - aws-rds:        Amazon RDS and Aurora PostgreSQL, through the `aws` CLI
          - aws-cloudwatch: RDS/Aurora logs exported to `CloudWatch` Logs (EXPERIMENTAL)
          - azure-flexible: Azure Database for PostgreSQL Flexible Server, through `az` (EXPERIMENTAL)
          - gcp-cloudsql:   Google Cloud SQL for PostgreSQL, through `gcloud` (EXPERIMENTAL)
          - aiven:          Aiven for PostgreSQL, through `avn` (EXPERIMENTAL)
          - heroku:         Heroku Postgres, through `heroku`: recent lines only (EXPERIMENTAL)
          - sql:            The server's own log directory over SQL (EXPERIMENTAL)

      --instance <INSTANCE>
          The instance: RDS instance identifier, Azure server name, Cloud SQL instance, Aiven service, or Heroku app

      --region <REGION>
          AWS region; default from the AWS CLI configuration

      --profile <PROFILE>
          AWS CLI profile

      --project <PROJECT>
          Google Cloud project (`gcp-cloudsql`) or Aiven project (`aiven`)

      --resource-group <RESOURCE_GROUP>
          Azure resource group (`azure-flexible`)

      --subscription <SUBSCRIPTION>
          Azure subscription (`azure-flexible`)

      --log-group <LOG_GROUP>
          `CloudWatch` log group (`aws-cloudwatch`); default `/aws/rds/instance/<instance>/postgresql`

      --bin <BIN>
          Path of the vendor CLI executable (default: `aws`, `az`, `gcloud`, `avn` or `heroku`)

      --conn <CONN>
          Connection for `--provider sql`
          
          [env: PGLOUPE_DSN]

      --password-env <PASSWORD_ENV>
          Environment variable that holds the password (default `PGPASSWORD`)

      --sslrootcert <SSLROOTCERT>
          CA certificate (PEM) to trust

      --since <SINCE>
          Start: `30m`, `24h`, `7d`, `2w` back from now, or an RFC 3339 time or `YYYY-MM-DD` (UTC)
          
          [default: 24h]

      --out <OUT>
          Directory the files go to (created if missing); a rerun continues where this stopped

  -h, --help
          Print help (see a summary with '-h')
```

## `pgloupe report`

```text
One report from logs and, optionally, two `pg_stat_statements` snapshots: HTML, JSON or text, with the two joined per statement

Usage: pgloupe report [OPTIONS] [FILES]...

Arguments:
  [FILES]...
          Log files (csvlog, jsonlog or stderr; compression is detected automatically), analyzed on the fly

Options:
      --store <STORE>
          A store written by `logs ingest`, instead of log files

      --since <SINCE>
          With `--store`: start of the log window (RFC 3339 or `YYYY-MM-DD`, UTC). Defaults to the snapshot interval; rounded down to the hour

      --until <UNTIL>
          With `--store`: end of the log window; rounded up to the hour

      --before <BEFORE>
          Older `pg_stat_statements` snapshot (CSV export)

      --after <AFTER>
          Newer snapshot

      --before-info <BEFORE_INFO>
          `pg_stat_statements_info` CSV taken with the older snapshot

      --after-info <AFTER_INFO>
          `pg_stat_statements_info` CSV taken with the newer snapshot

      --before-db <BEFORE_DB>
          `pg_stat_database` CSV taken with the older snapshot; found beside it when it is named `pg_stat_database_<time>.csv`

      --after-db <AFTER_DB>
          `pg_stat_database` CSV taken with the newer snapshot

      --format <FORMAT>
          Log format of `FILES`; `auto` looks at each file's first line

          Possible values:
          - auto
          - csv
          - json
          - stderr
          - pgbouncer: The pgBouncer log (experimental)
          
          [default: auto]

      --prefix <PREFIX>
          The server's `log_line_prefix` for `FILES`; inferred when omitted

      --log-tz <LOG_TZ>
          Offset for timezone abbreviations the log does not spell out, e.g. `-03` or `+05:30`

      --max-fingerprints <MAX_FINGERPRINTS>
          Distinct statements kept before the rest fold into one overflow row (`FILES`)
          
          [default: 50000]

      --threads <THREADS>
          Worker threads for `FILES` (default: all cores)

      --by <BY>
          Metric that orders the statement table

          Possible values:
          - total: Total execution time
          - calls: Number of calls
          - p95:   95th percentile of the duration
          - max:   Slowest single call
          
          [default: total]

      --top <TOP>
          Statements to list (`all` for every statement)
          
          [default: 20]

      --bucket <BUCKET>
          Timeline resolution: `30s`, `1m`, `1h` (a multiple of one minute with `--store`)
          
          [default: 1m]

      --output <OUTPUT>
          Report format

          Possible values:
          - text
          - json
          - html: One self-contained HTML file
          - csv:  The statement table, one row per statement
          
          [default: text]

      --baseline <BASELINE>
          Compare with an earlier period of the store: `previous` (the window just before this one, as long), or give `--baseline-since` and `--baseline-until`

          Possible values:
          - previous: The window just before this one, as long

      --baseline-since <BASELINE_SINCE>
          Start of the baseline window (needs `--baseline-until`)

      --baseline-until <BASELINE_UNTIL>
          End of the baseline window

      --settings <SETTINGS>
          `pg_settings` as CSV (`name,setting`, the export of `SELECT * FROM pg_settings`): the report then says exactly which settings limit it. `snap` writes one next to each snapshot, and it is picked up from beside `--after` when this is not given

      --vendor <VENDOR>
          Where the server runs, to word advice about settings (`rds`; detected when it can be)

      --display-tz <DISPLAY_TZ>
          Show times in this IANA zone in the HTML dashboard (default UTC); JSON and text stay UTC

      --out <OUT>
          Write the report here instead of to standard output. With several `--output` formats this is a path prefix: `--output html,json --out report` writes `report.html` and `report.json`

      --slowest <SLOWEST>
          Slowest single executions to keep and list (default 20)

      --params
          Keep the bind parameters of those executions; they can hold personal data

      --database <DATABASES>
          Only records of these databases (repeat or comma-separate)

      --user <USERS>
          Only records of these users

      --application <APPLICATIONS>
          Only records of these applications (`application_name`)

      --host <HOSTS>
          Only records from these client hosts

      --exclude-database <EXCLUDE_DATABASES>
          Drop records of these databases

      --exclude-user <EXCLUDE_USERS>
          Drop records of these users

      --exclude-application <EXCLUDE_APPLICATIONS>
          Drop records of these applications

      --exclude-host <EXCLUDE_HOSTS>
          Drop records from these client hosts

      --include-query <INCLUDE_QUERY>
          Only records whose message or statement matches this regular expression

      --exclude-query <EXCLUDE_QUERY>
          Drop records whose message or statement matches this regular expression

      --pid <PIDS>
          Only records of these backend process ids

      --exclude-message <EXCLUDE_MESSAGE>
          Drop records whose message matches this regular expression (noise such as health checks)

      --between <BETWEEN>
          Only records inside this time of day, as `HH:MM-HH:MM` (wraps past midnight)

      --exclude-hours <EXCLUDE_HOURS>
          Drop records in these hours of the day, as `5-7` (hours 5 to 7 inclusive); repeatable

      --weekdays <WEEKDAYS>
          Only records on these days: `mon-fri`, `sat,sun`

      --tz <TZ>
          Zone for `--between`, `--exclude-hours` and `--weekdays`: an IANA name or `UTC` (default)

      --anonymize
          Make the report safe to share: users, databases, applications and hosts become stable pseudonyms, and literals, emails and addresses are removed from every text

      --anonymize-key <ANONYMIZE_KEY>
          Secret that decides the pseudonyms (default: none, so common names are guessable)
          
          [env: PGLOUPE_ANONYMIZE_KEY]

      --lc-messages <LC_MESSAGES>
          The language of the server's messages (`lc_messages`): `auto` (default) looks at the log, `en` reads English, or one of `pt_BR`, `es`, `fr`, `de`
          
          [default: auto]

      --messages-file <MESSAGES_FILE>
          A translation table for another language: lines of `locale<TAB>english<TAB>translated`, as in the built-in one

  -h, --help
          Print help (see a summary with '-h')
```

## `pgloupe snap`

```text
Take a `pg_stat_statements` snapshot from a live server (read-only; never resets anything)

Usage: pgloupe snap [OPTIONS] --conn <CONN> --out <OUT>

Options:
      --conn <CONN>
          Connection string: `postgres://user@host:5432/db` or `key=value` pairs. The password comes from the string, `PGPASSWORD` (or `--password-env`), or `~/.pgpass`; prefer those to putting it on the command line
          
          [env: PGLOUPE_DSN]

      --password-env <PASSWORD_ENV>
          Environment variable that holds the password (default `PGPASSWORD`)

      --sslrootcert <SSLROOTCERT>
          CA certificate (PEM) to trust; turns on full certificate and host name checking. For RDS, the regional bundle from AWS

      --out <OUT>
          Directory the snapshot files go to (created if missing)

  -h, --help
          Print help
```

## `pgloupe doctor`

```text
Check what the server and the role lack for a useful report, and how to fix it

Usage: pgloupe doctor [OPTIONS] --conn <CONN>

Options:
      --conn <CONN>
          Connection string: `postgres://user@host:5432/db` or `key=value` pairs. The password comes from the string, `PGPASSWORD` (or `--password-env`), or `~/.pgpass`; prefer those to putting it on the command line
          
          [env: PGLOUPE_DSN]

      --password-env <PASSWORD_ENV>
          Environment variable that holds the password (default `PGPASSWORD`)

      --sslrootcert <SSLROOTCERT>
          CA certificate (PEM) to trust; turns on full certificate and host name checking. For RDS, the regional bundle from AWS

      --vendor <VENDOR>
          Say where settings live for this vendor

          Possible values:
          - rds: Amazon RDS and Aurora PostgreSQL: settings live in the parameter group

      --output <OUTPUT>
          Output format

          Possible values:
          - text: Human-readable
          - json: One JSON document
          
          [default: text]

  -h, --help
          Print help (see a summary with '-h')
```

## `pgloupe export`

```text
Check the server, take a snapshot and fetch logs into one directory; optionally the report

Usage: pgloupe export [OPTIONS] --conn <CONN> --provider <PROVIDER> --out <OUT>

Options:
      --conn <CONN>
          Connection string: `postgres://user@host:5432/db` or `key=value` pairs. The password comes from the string, `PGPASSWORD` (or `--password-env`), or `~/.pgpass`; prefer those to putting it on the command line
          
          [env: PGLOUPE_DSN]

      --password-env <PASSWORD_ENV>
          Environment variable that holds the password (default `PGPASSWORD`)

      --sslrootcert <SSLROOTCERT>
          CA certificate (PEM) to trust; turns on full certificate and host name checking. For RDS, the regional bundle from AWS

      --provider <PROVIDER>
          Log source

          Possible values:
          - aws-rds:        Amazon RDS and Aurora PostgreSQL, through the `aws` CLI
          - aws-cloudwatch: RDS/Aurora logs exported to `CloudWatch` Logs (EXPERIMENTAL)
          - azure-flexible: Azure Database for PostgreSQL Flexible Server, through `az` (EXPERIMENTAL)
          - gcp-cloudsql:   Google Cloud SQL for PostgreSQL, through `gcloud` (EXPERIMENTAL)
          - aiven:          Aiven for PostgreSQL, through `avn` (EXPERIMENTAL)
          - heroku:         Heroku Postgres, through `heroku`: recent lines only (EXPERIMENTAL)
          - sql:            The server's own log directory over SQL (EXPERIMENTAL)

      --instance <INSTANCE>
          The instance: RDS instance identifier, Azure server name, Cloud SQL instance, Aiven service, or Heroku app

      --region <REGION>
          AWS region; default from the AWS CLI configuration

      --profile <PROFILE>
          AWS CLI profile

      --project <PROJECT>
          Google Cloud project (`gcp-cloudsql`) or Aiven project (`aiven`)

      --resource-group <RESOURCE_GROUP>
          Azure resource group (`azure-flexible`)

      --subscription <SUBSCRIPTION>
          Azure subscription (`azure-flexible`)

      --log-group <LOG_GROUP>
          `CloudWatch` log group (`aws-cloudwatch`); default `/aws/rds/instance/<instance>/postgresql`

      --bin <BIN>
          Path of the vendor CLI executable (default: `aws`, `az`, `gcloud`, `avn` or `heroku`)

      --since <SINCE>
          How far back to fetch logs (see `logs fetch --since`)
          
          [default: 24h]

      --out <OUT>
          Directory for everything: `snapshots/`, `logs/` and the fetch state

      --report <REPORT>
          Also write the report here (HTML) from the logs and the two newest snapshots

      --prefix <PREFIX>
          The server's `log_line_prefix`, for stderr logs whose prefix cannot be inferred

  -h, --help
          Print help (see a summary with '-h')
```

## `pgloupe watch`

```text
Keep a report fresh from a live server and serve it locally

Usage: pgloupe watch [OPTIONS] --conn <CONN> --provider <PROVIDER>

Options:
      --conn <CONN>
          Connection string: `postgres://user@host:5432/db` or `key=value` pairs. The password comes from the string, `PGPASSWORD` (or `--password-env`), or `~/.pgpass`; prefer those to putting it on the command line
          
          [env: PGLOUPE_DSN]

      --password-env <PASSWORD_ENV>
          Environment variable that holds the password (default `PGPASSWORD`)

      --sslrootcert <SSLROOTCERT>
          CA certificate (PEM) to trust; turns on full certificate and host name checking. For RDS, the regional bundle from AWS

      --provider <PROVIDER>
          Log source

          Possible values:
          - aws-rds:        Amazon RDS and Aurora PostgreSQL, through the `aws` CLI
          - aws-cloudwatch: RDS/Aurora logs exported to `CloudWatch` Logs (EXPERIMENTAL)
          - azure-flexible: Azure Database for PostgreSQL Flexible Server, through `az` (EXPERIMENTAL)
          - gcp-cloudsql:   Google Cloud SQL for PostgreSQL, through `gcloud` (EXPERIMENTAL)
          - aiven:          Aiven for PostgreSQL, through `avn` (EXPERIMENTAL)
          - heroku:         Heroku Postgres, through `heroku`: recent lines only (EXPERIMENTAL)
          - sql:            The server's own log directory over SQL (EXPERIMENTAL)

      --instance <INSTANCE>
          The instance: RDS instance identifier, Azure server name, Cloud SQL instance, Aiven service, or Heroku app

      --region <REGION>
          AWS region; default from the AWS CLI configuration

      --profile <PROFILE>
          AWS CLI profile

      --project <PROJECT>
          Google Cloud project (`gcp-cloudsql`) or Aiven project (`aiven`)

      --resource-group <RESOURCE_GROUP>
          Azure resource group (`azure-flexible`)

      --subscription <SUBSCRIPTION>
          Azure subscription (`azure-flexible`)

      --log-group <LOG_GROUP>
          `CloudWatch` log group (`aws-cloudwatch`); default `/aws/rds/instance/<instance>/postgresql`

      --bin <BIN>
          Path of the vendor CLI executable (default: `aws`, `az`, `gcloud`, `avn` or `heroku`)

      --interval <INTERVAL>
          Time between updates (`30s`, `5m`, `1h`)
          
          [default: 5m]

      --window <WINDOW>
          How far back the report looks (`24h`, `7d`)
          
          [default: 24h]

      --listen <LISTEN>
          Address to serve the dashboard on (loopback only unless `--allow-remote`)
          
          [default: 127.0.0.1:8080]

      --allow-remote
          Allow `--listen` on an address other machines can reach. The page holds statement text

      --state-dir <STATE_DIR>
          Where snapshots, fetched logs and the store are kept
          
          [default: pgloupe-watch]

      --keep-snapshots <KEEP_SNAPSHOTS>
          Snapshots to keep (288 is 24 hours at 5 minutes)
          
          [default: 288]

      --prefix <PREFIX>
          The server's `log_line_prefix`, if the logs are stderr and it cannot be inferred

      --out <OUT>
          Also write the HTML here after every update

      --once
          Run one cycle and exit instead of looping and serving

  -h, --help
          Print help (see a summary with '-h')
```
