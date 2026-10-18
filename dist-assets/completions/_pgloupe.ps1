
using namespace System.Management.Automation
using namespace System.Management.Automation.Language

Register-ArgumentCompleter -Native -CommandName 'pgloupe' -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)

    $commandElements = $commandAst.CommandElements
    $command = @(
        'pgloupe'
        for ($i = 1; $i -lt $commandElements.Count; $i++) {
            $element = $commandElements[$i]
            if ($element -isnot [StringConstantExpressionAst] -or
                $element.StringConstantType -ne [StringConstantType]::BareWord -or
                $element.Value.StartsWith('-') -or
                $element.Value -eq $wordToComplete) {
                break
        }
        $element.Value
    }) -join ';'

    $completions = @(switch ($command) {
        'pgloupe' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('-V', '-V ', [CompletionResultType]::ParameterName, 'Print version')
            [CompletionResult]::new('--version', '--version', [CompletionResultType]::ParameterName, 'Print version')
            [CompletionResult]::new('stats', 'stats', [CompletionResultType]::ParameterValue, 'Work with `pg_stat_statements` snapshots')
            [CompletionResult]::new('logs', 'logs', [CompletionResultType]::ParameterValue, 'Work with Postgres server logs')
            [CompletionResult]::new('report', 'report', [CompletionResultType]::ParameterValue, 'One report from logs and, optionally, two `pg_stat_statements` snapshots: HTML, JSON, text or CSV, with the two joined per statement')
            [CompletionResult]::new('snap', 'snap', [CompletionResultType]::ParameterValue, 'Take a `pg_stat_statements` snapshot from a live server (read-only; never resets anything)')
            [CompletionResult]::new('doctor', 'doctor', [CompletionResultType]::ParameterValue, 'Check what the server and the role lack for a useful report, and how to fix it')
            [CompletionResult]::new('export', 'export', [CompletionResultType]::ParameterValue, 'Check the server, take a snapshot and fetch logs into one directory; optionally the report')
            [CompletionResult]::new('watch', 'watch', [CompletionResultType]::ParameterValue, 'Keep a report fresh from a live server and serve it locally')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'pgloupe;stats' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('diff', 'diff', [CompletionResultType]::ParameterValue, 'Compare two snapshot CSV exports and rank the queries that changed')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'pgloupe;stats;diff' {
            [CompletionResult]::new('--before-info', '--before-info', [CompletionResultType]::ParameterName, '`pg_stat_statements_info` CSV taken with the older snapshot')
            [CompletionResult]::new('--after-info', '--after-info', [CompletionResultType]::ParameterName, '`pg_stat_statements_info` CSV taken with the newer snapshot')
            [CompletionResult]::new('--by', '--by', [CompletionResultType]::ParameterName, 'Metric that orders the table')
            [CompletionResult]::new('--top', '--top', [CompletionResultType]::ParameterName, 'Rows to show (`all` for every statement)')
            [CompletionResult]::new('--min-calls', '--min-calls', [CompletionResultType]::ParameterName, 'With `--by mean`, skip queries with fewer calls than this in the window')
            [CompletionResult]::new('--format', '--format', [CompletionResultType]::ParameterName, 'Output format')
            [CompletionResult]::new('--wide', '--wide', [CompletionResultType]::ParameterName, 'Show full query text in the table')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            break
        }
        'pgloupe;stats;help' {
            [CompletionResult]::new('diff', 'diff', [CompletionResultType]::ParameterValue, 'Compare two snapshot CSV exports and rank the queries that changed')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'pgloupe;stats;help;diff' {
            break
        }
        'pgloupe;stats;help;help' {
            break
        }
        'pgloupe;logs' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('dump', 'dump', [CompletionResultType]::ParameterValue, 'Decode a log (csvlog, jsonlog or stderr; compression is detected) and print each record as JSON')
            [CompletionResult]::new('analyze', 'analyze', [CompletionResultType]::ParameterValue, 'Summarize one or more logs: statements with percentiles, errors, locks, temp files, connections, checkpoints and autovacuum')
            [CompletionResult]::new('ingest', 'ingest', [CompletionResultType]::ParameterValue, 'Add logs to a store; files already ingested are skipped and grown ones replace their old version')
            [CompletionResult]::new('report', 'report', [CompletionResultType]::ParameterValue, 'Report on a time window of a store')
            [CompletionResult]::new('fetch', 'fetch', [CompletionResultType]::ParameterValue, 'Download a time range of log files from a managed service or server (resumable)')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'pgloupe;logs;dump' {
            [CompletionResult]::new('--format', '--format', [CompletionResultType]::ParameterName, 'Log format; `auto` looks at the first line')
            [CompletionResult]::new('--prefix', '--prefix', [CompletionResultType]::ParameterName, 'The server''s `log_line_prefix`; required for stderr logs')
            [CompletionResult]::new('--log-tz', '--log-tz', [CompletionResultType]::ParameterName, 'Offset for timezone abbreviations the log does not spell out, e.g. `-03` or `+05:30`')
            [CompletionResult]::new('--limit', '--limit', [CompletionResultType]::ParameterName, 'Stop after this many records')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            break
        }
        'pgloupe;logs;analyze' {
            [CompletionResult]::new('--format', '--format', [CompletionResultType]::ParameterName, 'Log format; `auto` looks at each file''s first line')
            [CompletionResult]::new('--prefix', '--prefix', [CompletionResultType]::ParameterName, 'The server''s `log_line_prefix`; inferred when omitted')
            [CompletionResult]::new('--log-tz', '--log-tz', [CompletionResultType]::ParameterName, 'Offset for timezone abbreviations the log does not spell out, e.g. `-03` or `+05:30`')
            [CompletionResult]::new('--by', '--by', [CompletionResultType]::ParameterName, 'Metric that orders the statement table')
            [CompletionResult]::new('--top', '--top', [CompletionResultType]::ParameterName, 'Statements to list (`all` for every statement)')
            [CompletionResult]::new('--bucket', '--bucket', [CompletionResultType]::ParameterName, 'Timeline resolution: `30s`, `1m`, `1h`')
            [CompletionResult]::new('--max-fingerprints', '--max-fingerprints', [CompletionResultType]::ParameterName, 'Distinct statements kept before the rest fold into one overflow row')
            [CompletionResult]::new('--threads', '--threads', [CompletionResultType]::ParameterName, 'Worker threads (default: all cores)')
            [CompletionResult]::new('--output', '--output', [CompletionResultType]::ParameterName, 'Report format')
            [CompletionResult]::new('--slowest', '--slowest', [CompletionResultType]::ParameterName, 'Slowest single executions to keep and list (default 20)')
            [CompletionResult]::new('--database', '--database', [CompletionResultType]::ParameterName, 'Only records of these databases (repeat or comma-separate)')
            [CompletionResult]::new('--user', '--user', [CompletionResultType]::ParameterName, 'Only records of these users')
            [CompletionResult]::new('--application', '--application', [CompletionResultType]::ParameterName, 'Only records of these applications (`application_name`)')
            [CompletionResult]::new('--host', '--host', [CompletionResultType]::ParameterName, 'Only records from these client hosts')
            [CompletionResult]::new('--exclude-database', '--exclude-database', [CompletionResultType]::ParameterName, 'Drop records of these databases')
            [CompletionResult]::new('--exclude-user', '--exclude-user', [CompletionResultType]::ParameterName, 'Drop records of these users')
            [CompletionResult]::new('--exclude-application', '--exclude-application', [CompletionResultType]::ParameterName, 'Drop records of these applications')
            [CompletionResult]::new('--exclude-host', '--exclude-host', [CompletionResultType]::ParameterName, 'Drop records from these client hosts')
            [CompletionResult]::new('--include-query', '--include-query', [CompletionResultType]::ParameterName, 'Only records whose message or statement matches this regular expression')
            [CompletionResult]::new('--exclude-query', '--exclude-query', [CompletionResultType]::ParameterName, 'Drop records whose message or statement matches this regular expression')
            [CompletionResult]::new('--pid', '--pid', [CompletionResultType]::ParameterName, 'Only records of these backend process ids')
            [CompletionResult]::new('--exclude-message', '--exclude-message', [CompletionResultType]::ParameterName, 'Drop records whose message matches this regular expression (noise such as health checks)')
            [CompletionResult]::new('--between', '--between', [CompletionResultType]::ParameterName, 'Only records inside this time of day, as `HH:MM-HH:MM` (wraps past midnight)')
            [CompletionResult]::new('--exclude-hours', '--exclude-hours', [CompletionResultType]::ParameterName, 'Drop records in these hours of the day, as `5-7` (hours 5 to 7 inclusive); repeatable')
            [CompletionResult]::new('--weekdays', '--weekdays', [CompletionResultType]::ParameterName, 'Only records on these days: `mon-fri`, `sat,sun`')
            [CompletionResult]::new('--tz', '--tz', [CompletionResultType]::ParameterName, 'Zone for `--between`, `--exclude-hours` and `--weekdays`: an IANA name or `UTC` (default)')
            [CompletionResult]::new('--anonymize-key', '--anonymize-key', [CompletionResultType]::ParameterName, 'Secret that decides the pseudonyms (default: none, so common names are guessable)')
            [CompletionResult]::new('--lc-messages', '--lc-messages', [CompletionResultType]::ParameterName, 'The language of the server''s messages (`lc_messages`): `auto` (default) looks at the log, `en` reads English, or one of `pt_BR`, `es`, `fr`, `de`')
            [CompletionResult]::new('--messages-file', '--messages-file', [CompletionResultType]::ParameterName, 'A translation table for another language: lines of `locale<TAB>english<TAB>translated`, as in the built-in one')
            [CompletionResult]::new('--params', '--params', [CompletionResultType]::ParameterName, 'Keep the bind parameters of those executions; they can hold personal data')
            [CompletionResult]::new('--anonymize', '--anonymize', [CompletionResultType]::ParameterName, 'Make the report safe to share: users, databases, applications and hosts become stable pseudonyms, and literals, emails and addresses are removed from every text')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            break
        }
        'pgloupe;logs;ingest' {
            [CompletionResult]::new('--store', '--store', [CompletionResultType]::ParameterName, 'The store file; created when missing')
            [CompletionResult]::new('--format', '--format', [CompletionResultType]::ParameterName, 'Log format; `auto` looks at each file''s first line')
            [CompletionResult]::new('--prefix', '--prefix', [CompletionResultType]::ParameterName, 'The server''s `log_line_prefix`; inferred when omitted')
            [CompletionResult]::new('--log-tz', '--log-tz', [CompletionResultType]::ParameterName, 'Offset for timezone abbreviations the log does not spell out, e.g. `-03` or `+05:30`')
            [CompletionResult]::new('--max-fingerprints', '--max-fingerprints', [CompletionResultType]::ParameterName, 'Distinct statements kept per hour before the rest fold into one overflow row')
            [CompletionResult]::new('--threads', '--threads', [CompletionResultType]::ParameterName, 'Worker threads (default: all cores)')
            [CompletionResult]::new('--slowest', '--slowest', [CompletionResultType]::ParameterName, 'Slowest single executions to keep and list (default 20)')
            [CompletionResult]::new('--database', '--database', [CompletionResultType]::ParameterName, 'Only records of these databases (repeat or comma-separate)')
            [CompletionResult]::new('--user', '--user', [CompletionResultType]::ParameterName, 'Only records of these users')
            [CompletionResult]::new('--application', '--application', [CompletionResultType]::ParameterName, 'Only records of these applications (`application_name`)')
            [CompletionResult]::new('--host', '--host', [CompletionResultType]::ParameterName, 'Only records from these client hosts')
            [CompletionResult]::new('--exclude-database', '--exclude-database', [CompletionResultType]::ParameterName, 'Drop records of these databases')
            [CompletionResult]::new('--exclude-user', '--exclude-user', [CompletionResultType]::ParameterName, 'Drop records of these users')
            [CompletionResult]::new('--exclude-application', '--exclude-application', [CompletionResultType]::ParameterName, 'Drop records of these applications')
            [CompletionResult]::new('--exclude-host', '--exclude-host', [CompletionResultType]::ParameterName, 'Drop records from these client hosts')
            [CompletionResult]::new('--include-query', '--include-query', [CompletionResultType]::ParameterName, 'Only records whose message or statement matches this regular expression')
            [CompletionResult]::new('--exclude-query', '--exclude-query', [CompletionResultType]::ParameterName, 'Drop records whose message or statement matches this regular expression')
            [CompletionResult]::new('--pid', '--pid', [CompletionResultType]::ParameterName, 'Only records of these backend process ids')
            [CompletionResult]::new('--exclude-message', '--exclude-message', [CompletionResultType]::ParameterName, 'Drop records whose message matches this regular expression (noise such as health checks)')
            [CompletionResult]::new('--between', '--between', [CompletionResultType]::ParameterName, 'Only records inside this time of day, as `HH:MM-HH:MM` (wraps past midnight)')
            [CompletionResult]::new('--exclude-hours', '--exclude-hours', [CompletionResultType]::ParameterName, 'Drop records in these hours of the day, as `5-7` (hours 5 to 7 inclusive); repeatable')
            [CompletionResult]::new('--weekdays', '--weekdays', [CompletionResultType]::ParameterName, 'Only records on these days: `mon-fri`, `sat,sun`')
            [CompletionResult]::new('--tz', '--tz', [CompletionResultType]::ParameterName, 'Zone for `--between`, `--exclude-hours` and `--weekdays`: an IANA name or `UTC` (default)')
            [CompletionResult]::new('--anonymize-key', '--anonymize-key', [CompletionResultType]::ParameterName, 'Secret that decides the pseudonyms (default: none, so common names are guessable)')
            [CompletionResult]::new('--lc-messages', '--lc-messages', [CompletionResultType]::ParameterName, 'The language of the server''s messages (`lc_messages`): `auto` (default) looks at the log, `en` reads English, or one of `pt_BR`, `es`, `fr`, `de`')
            [CompletionResult]::new('--messages-file', '--messages-file', [CompletionResultType]::ParameterName, 'A translation table for another language: lines of `locale<TAB>english<TAB>translated`, as in the built-in one')
            [CompletionResult]::new('--force', '--force', [CompletionResultType]::ParameterName, 'Read and store files even when their content is already in the store')
            [CompletionResult]::new('--params', '--params', [CompletionResultType]::ParameterName, 'Keep the bind parameters of those executions; they can hold personal data')
            [CompletionResult]::new('--anonymize', '--anonymize', [CompletionResultType]::ParameterName, 'Make the report safe to share: users, databases, applications and hosts become stable pseudonyms, and literals, emails and addresses are removed from every text')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            break
        }
        'pgloupe;logs;report' {
            [CompletionResult]::new('--store', '--store', [CompletionResultType]::ParameterName, 'The store file written by `logs ingest`')
            [CompletionResult]::new('--slowest', '--slowest', [CompletionResultType]::ParameterName, 'Slowest executions to list, at most what the ingest kept (20 unless it was told more)')
            [CompletionResult]::new('--since', '--since', [CompletionResultType]::ParameterName, 'Start of the window (RFC 3339 or `YYYY-MM-DD`, UTC); rounded down to the hour')
            [CompletionResult]::new('--until', '--until', [CompletionResultType]::ParameterName, 'End of the window; rounded up to the hour')
            [CompletionResult]::new('--by', '--by', [CompletionResultType]::ParameterName, 'Metric that orders the statement table')
            [CompletionResult]::new('--top', '--top', [CompletionResultType]::ParameterName, 'Statements to list (`all` for every statement)')
            [CompletionResult]::new('--bucket', '--bucket', [CompletionResultType]::ParameterName, 'Timeline resolution: a multiple of one minute')
            [CompletionResult]::new('--output', '--output', [CompletionResultType]::ParameterName, 'Report format')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            break
        }
        'pgloupe;logs;fetch' {
            [CompletionResult]::new('--provider', '--provider', [CompletionResultType]::ParameterName, 'Log source')
            [CompletionResult]::new('--instance', '--instance', [CompletionResultType]::ParameterName, 'The instance: RDS instance identifier, Azure server name, Cloud SQL instance, Aiven service, or Heroku app')
            [CompletionResult]::new('--region', '--region', [CompletionResultType]::ParameterName, 'AWS region; default from the AWS CLI configuration')
            [CompletionResult]::new('--profile', '--profile', [CompletionResultType]::ParameterName, 'AWS CLI profile')
            [CompletionResult]::new('--project', '--project', [CompletionResultType]::ParameterName, 'Google Cloud project (`gcp-cloudsql`) or Aiven project (`aiven`)')
            [CompletionResult]::new('--resource-group', '--resource-group', [CompletionResultType]::ParameterName, 'Azure resource group (`azure-flexible`)')
            [CompletionResult]::new('--subscription', '--subscription', [CompletionResultType]::ParameterName, 'Azure subscription (`azure-flexible`)')
            [CompletionResult]::new('--log-group', '--log-group', [CompletionResultType]::ParameterName, '`CloudWatch` log group (`aws-cloudwatch`); default `/aws/rds/instance/<instance>/postgresql`')
            [CompletionResult]::new('--bin', '--bin', [CompletionResultType]::ParameterName, 'Path of the vendor CLI executable (default: `aws`, `az`, `gcloud`, `avn` or `heroku`)')
            [CompletionResult]::new('--jobs', '--jobs', [CompletionResultType]::ParameterName, 'Files downloaded at the same time (`aws-rds`; 1 to 16)')
            [CompletionResult]::new('--conn', '--conn', [CompletionResultType]::ParameterName, 'Connection for `--provider sql`')
            [CompletionResult]::new('--password-env', '--password-env', [CompletionResultType]::ParameterName, 'Environment variable that holds the password (default `PGPASSWORD`)')
            [CompletionResult]::new('--sslrootcert', '--sslrootcert', [CompletionResultType]::ParameterName, 'CA certificate (PEM) to trust')
            [CompletionResult]::new('--since', '--since', [CompletionResultType]::ParameterName, 'Start: `30m`, `24h`, `7d`, `2w` back from now, or an RFC 3339 time or `YYYY-MM-DD` (UTC)')
            [CompletionResult]::new('--out', '--out', [CompletionResultType]::ParameterName, 'Directory the files go to (created if missing); a rerun continues where this stopped')
            [CompletionResult]::new('--quiet', '--quiet', [CompletionResultType]::ParameterName, 'Do not print a line for each finished file')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            break
        }
        'pgloupe;logs;help' {
            [CompletionResult]::new('dump', 'dump', [CompletionResultType]::ParameterValue, 'Decode a log (csvlog, jsonlog or stderr; compression is detected) and print each record as JSON')
            [CompletionResult]::new('analyze', 'analyze', [CompletionResultType]::ParameterValue, 'Summarize one or more logs: statements with percentiles, errors, locks, temp files, connections, checkpoints and autovacuum')
            [CompletionResult]::new('ingest', 'ingest', [CompletionResultType]::ParameterValue, 'Add logs to a store; files already ingested are skipped and grown ones replace their old version')
            [CompletionResult]::new('report', 'report', [CompletionResultType]::ParameterValue, 'Report on a time window of a store')
            [CompletionResult]::new('fetch', 'fetch', [CompletionResultType]::ParameterValue, 'Download a time range of log files from a managed service or server (resumable)')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'pgloupe;logs;help;dump' {
            break
        }
        'pgloupe;logs;help;analyze' {
            break
        }
        'pgloupe;logs;help;ingest' {
            break
        }
        'pgloupe;logs;help;report' {
            break
        }
        'pgloupe;logs;help;fetch' {
            break
        }
        'pgloupe;logs;help;help' {
            break
        }
        'pgloupe;report' {
            [CompletionResult]::new('--store', '--store', [CompletionResultType]::ParameterName, 'A store written by `logs ingest`, instead of log files')
            [CompletionResult]::new('--since', '--since', [CompletionResultType]::ParameterName, 'With `--store`: start of the log window (RFC 3339 or `YYYY-MM-DD`, UTC). Defaults to the snapshot interval; rounded down to the hour')
            [CompletionResult]::new('--until', '--until', [CompletionResultType]::ParameterName, 'With `--store`: end of the log window; rounded up to the hour')
            [CompletionResult]::new('--before', '--before', [CompletionResultType]::ParameterName, 'Older `pg_stat_statements` snapshot (CSV export)')
            [CompletionResult]::new('--after', '--after', [CompletionResultType]::ParameterName, 'Newer snapshot')
            [CompletionResult]::new('--before-info', '--before-info', [CompletionResultType]::ParameterName, '`pg_stat_statements_info` CSV taken with the older snapshot')
            [CompletionResult]::new('--after-info', '--after-info', [CompletionResultType]::ParameterName, '`pg_stat_statements_info` CSV taken with the newer snapshot')
            [CompletionResult]::new('--before-db', '--before-db', [CompletionResultType]::ParameterName, '`pg_stat_database` CSV taken with the older snapshot; found beside it when it is named `pg_stat_database_<time>.csv`')
            [CompletionResult]::new('--after-db', '--after-db', [CompletionResultType]::ParameterName, '`pg_stat_database` CSV taken with the newer snapshot')
            [CompletionResult]::new('--format', '--format', [CompletionResultType]::ParameterName, 'Log format of `FILES`; `auto` looks at each file''s first line')
            [CompletionResult]::new('--prefix', '--prefix', [CompletionResultType]::ParameterName, 'The server''s `log_line_prefix` for `FILES`; inferred when omitted')
            [CompletionResult]::new('--log-tz', '--log-tz', [CompletionResultType]::ParameterName, 'Offset for timezone abbreviations the log does not spell out, e.g. `-03` or `+05:30`')
            [CompletionResult]::new('--max-fingerprints', '--max-fingerprints', [CompletionResultType]::ParameterName, 'Distinct statements kept before the rest fold into one overflow row (`FILES`)')
            [CompletionResult]::new('--threads', '--threads', [CompletionResultType]::ParameterName, 'Worker threads for `FILES` (default: all cores)')
            [CompletionResult]::new('--by', '--by', [CompletionResultType]::ParameterName, 'Metric that orders the statement table')
            [CompletionResult]::new('--top', '--top', [CompletionResultType]::ParameterName, 'Statements to list (`all` for every statement)')
            [CompletionResult]::new('--bucket', '--bucket', [CompletionResultType]::ParameterName, 'Timeline resolution: `30s`, `1m`, `1h` (a multiple of one minute with `--store`)')
            [CompletionResult]::new('--output', '--output', [CompletionResultType]::ParameterName, 'Report format')
            [CompletionResult]::new('--baseline', '--baseline', [CompletionResultType]::ParameterName, 'Compare with an earlier period of the store: `previous` (the window just before this one, as long), or give `--baseline-since` and `--baseline-until`')
            [CompletionResult]::new('--baseline-since', '--baseline-since', [CompletionResultType]::ParameterName, 'Start of the baseline window (needs `--baseline-until`)')
            [CompletionResult]::new('--baseline-until', '--baseline-until', [CompletionResultType]::ParameterName, 'End of the baseline window')
            [CompletionResult]::new('--settings', '--settings', [CompletionResultType]::ParameterName, '`pg_settings` as CSV (`name,setting`, the export of `SELECT * FROM pg_settings`): the report then says exactly which settings limit it. `snap` writes one next to each snapshot, and it is picked up from beside `--after` when this is not given')
            [CompletionResult]::new('--vendor', '--vendor', [CompletionResultType]::ParameterName, 'Where the server runs, to word advice about settings (`rds`; detected when it can be)')
            [CompletionResult]::new('--display-tz', '--display-tz', [CompletionResultType]::ParameterName, 'Show times in this IANA zone in the HTML dashboard (default UTC); JSON and text stay UTC')
            [CompletionResult]::new('--out', '--out', [CompletionResultType]::ParameterName, 'Write the report here instead of to standard output. With several `--output` formats this is a path prefix: `--output html,json --out report` writes `report.html` and `report.json`')
            [CompletionResult]::new('--slowest', '--slowest', [CompletionResultType]::ParameterName, 'Slowest single executions to keep and list (default 20)')
            [CompletionResult]::new('--database', '--database', [CompletionResultType]::ParameterName, 'Only records of these databases (repeat or comma-separate)')
            [CompletionResult]::new('--user', '--user', [CompletionResultType]::ParameterName, 'Only records of these users')
            [CompletionResult]::new('--application', '--application', [CompletionResultType]::ParameterName, 'Only records of these applications (`application_name`)')
            [CompletionResult]::new('--host', '--host', [CompletionResultType]::ParameterName, 'Only records from these client hosts')
            [CompletionResult]::new('--exclude-database', '--exclude-database', [CompletionResultType]::ParameterName, 'Drop records of these databases')
            [CompletionResult]::new('--exclude-user', '--exclude-user', [CompletionResultType]::ParameterName, 'Drop records of these users')
            [CompletionResult]::new('--exclude-application', '--exclude-application', [CompletionResultType]::ParameterName, 'Drop records of these applications')
            [CompletionResult]::new('--exclude-host', '--exclude-host', [CompletionResultType]::ParameterName, 'Drop records from these client hosts')
            [CompletionResult]::new('--include-query', '--include-query', [CompletionResultType]::ParameterName, 'Only records whose message or statement matches this regular expression')
            [CompletionResult]::new('--exclude-query', '--exclude-query', [CompletionResultType]::ParameterName, 'Drop records whose message or statement matches this regular expression')
            [CompletionResult]::new('--pid', '--pid', [CompletionResultType]::ParameterName, 'Only records of these backend process ids')
            [CompletionResult]::new('--exclude-message', '--exclude-message', [CompletionResultType]::ParameterName, 'Drop records whose message matches this regular expression (noise such as health checks)')
            [CompletionResult]::new('--between', '--between', [CompletionResultType]::ParameterName, 'Only records inside this time of day, as `HH:MM-HH:MM` (wraps past midnight)')
            [CompletionResult]::new('--exclude-hours', '--exclude-hours', [CompletionResultType]::ParameterName, 'Drop records in these hours of the day, as `5-7` (hours 5 to 7 inclusive); repeatable')
            [CompletionResult]::new('--weekdays', '--weekdays', [CompletionResultType]::ParameterName, 'Only records on these days: `mon-fri`, `sat,sun`')
            [CompletionResult]::new('--tz', '--tz', [CompletionResultType]::ParameterName, 'Zone for `--between`, `--exclude-hours` and `--weekdays`: an IANA name or `UTC` (default)')
            [CompletionResult]::new('--anonymize-key', '--anonymize-key', [CompletionResultType]::ParameterName, 'Secret that decides the pseudonyms (default: none, so common names are guessable)')
            [CompletionResult]::new('--lc-messages', '--lc-messages', [CompletionResultType]::ParameterName, 'The language of the server''s messages (`lc_messages`): `auto` (default) looks at the log, `en` reads English, or one of `pt_BR`, `es`, `fr`, `de`')
            [CompletionResult]::new('--messages-file', '--messages-file', [CompletionResultType]::ParameterName, 'A translation table for another language: lines of `locale<TAB>english<TAB>translated`, as in the built-in one')
            [CompletionResult]::new('--params', '--params', [CompletionResultType]::ParameterName, 'Keep the bind parameters of those executions; they can hold personal data')
            [CompletionResult]::new('--anonymize', '--anonymize', [CompletionResultType]::ParameterName, 'Make the report safe to share: users, databases, applications and hosts become stable pseudonyms, and literals, emails and addresses are removed from every text')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            break
        }
        'pgloupe;snap' {
            [CompletionResult]::new('--conn', '--conn', [CompletionResultType]::ParameterName, 'Connection string: `postgres://user@host:5432/db` or `key=value` pairs. The password comes from the string, `PGPASSWORD` (or `--password-env`), or `~/.pgpass`; prefer those to putting it on the command line')
            [CompletionResult]::new('--password-env', '--password-env', [CompletionResultType]::ParameterName, 'Environment variable that holds the password (default `PGPASSWORD`)')
            [CompletionResult]::new('--sslrootcert', '--sslrootcert', [CompletionResultType]::ParameterName, 'CA certificate (PEM) to trust; turns on full certificate and host name checking. For RDS, the regional bundle from AWS')
            [CompletionResult]::new('--out', '--out', [CompletionResultType]::ParameterName, 'Directory the snapshot files go to (created if missing)')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'pgloupe;doctor' {
            [CompletionResult]::new('--conn', '--conn', [CompletionResultType]::ParameterName, 'Connection string: `postgres://user@host:5432/db` or `key=value` pairs. The password comes from the string, `PGPASSWORD` (or `--password-env`), or `~/.pgpass`; prefer those to putting it on the command line')
            [CompletionResult]::new('--password-env', '--password-env', [CompletionResultType]::ParameterName, 'Environment variable that holds the password (default `PGPASSWORD`)')
            [CompletionResult]::new('--sslrootcert', '--sslrootcert', [CompletionResultType]::ParameterName, 'CA certificate (PEM) to trust; turns on full certificate and host name checking. For RDS, the regional bundle from AWS')
            [CompletionResult]::new('--vendor', '--vendor', [CompletionResultType]::ParameterName, 'Say where settings live for this vendor')
            [CompletionResult]::new('--output', '--output', [CompletionResultType]::ParameterName, 'Output format')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            break
        }
        'pgloupe;export' {
            [CompletionResult]::new('--conn', '--conn', [CompletionResultType]::ParameterName, 'Connection string: `postgres://user@host:5432/db` or `key=value` pairs. The password comes from the string, `PGPASSWORD` (or `--password-env`), or `~/.pgpass`; prefer those to putting it on the command line')
            [CompletionResult]::new('--password-env', '--password-env', [CompletionResultType]::ParameterName, 'Environment variable that holds the password (default `PGPASSWORD`)')
            [CompletionResult]::new('--sslrootcert', '--sslrootcert', [CompletionResultType]::ParameterName, 'CA certificate (PEM) to trust; turns on full certificate and host name checking. For RDS, the regional bundle from AWS')
            [CompletionResult]::new('--provider', '--provider', [CompletionResultType]::ParameterName, 'Log source')
            [CompletionResult]::new('--instance', '--instance', [CompletionResultType]::ParameterName, 'The instance: RDS instance identifier, Azure server name, Cloud SQL instance, Aiven service, or Heroku app')
            [CompletionResult]::new('--region', '--region', [CompletionResultType]::ParameterName, 'AWS region; default from the AWS CLI configuration')
            [CompletionResult]::new('--profile', '--profile', [CompletionResultType]::ParameterName, 'AWS CLI profile')
            [CompletionResult]::new('--project', '--project', [CompletionResultType]::ParameterName, 'Google Cloud project (`gcp-cloudsql`) or Aiven project (`aiven`)')
            [CompletionResult]::new('--resource-group', '--resource-group', [CompletionResultType]::ParameterName, 'Azure resource group (`azure-flexible`)')
            [CompletionResult]::new('--subscription', '--subscription', [CompletionResultType]::ParameterName, 'Azure subscription (`azure-flexible`)')
            [CompletionResult]::new('--log-group', '--log-group', [CompletionResultType]::ParameterName, '`CloudWatch` log group (`aws-cloudwatch`); default `/aws/rds/instance/<instance>/postgresql`')
            [CompletionResult]::new('--bin', '--bin', [CompletionResultType]::ParameterName, 'Path of the vendor CLI executable (default: `aws`, `az`, `gcloud`, `avn` or `heroku`)')
            [CompletionResult]::new('--jobs', '--jobs', [CompletionResultType]::ParameterName, 'Files downloaded at the same time (`aws-rds`; 1 to 16)')
            [CompletionResult]::new('--since', '--since', [CompletionResultType]::ParameterName, 'How far back to fetch logs (see `logs fetch --since`)')
            [CompletionResult]::new('--out', '--out', [CompletionResultType]::ParameterName, 'Directory for everything: `snapshots/`, `logs/` and the fetch state')
            [CompletionResult]::new('--report', '--report', [CompletionResultType]::ParameterName, 'Also write the report here (HTML) from the logs and the two newest snapshots')
            [CompletionResult]::new('--prefix', '--prefix', [CompletionResultType]::ParameterName, 'The server''s `log_line_prefix`, for stderr logs whose prefix cannot be inferred')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            break
        }
        'pgloupe;watch' {
            [CompletionResult]::new('--conn', '--conn', [CompletionResultType]::ParameterName, 'Connection string: `postgres://user@host:5432/db` or `key=value` pairs. The password comes from the string, `PGPASSWORD` (or `--password-env`), or `~/.pgpass`; prefer those to putting it on the command line')
            [CompletionResult]::new('--password-env', '--password-env', [CompletionResultType]::ParameterName, 'Environment variable that holds the password (default `PGPASSWORD`)')
            [CompletionResult]::new('--sslrootcert', '--sslrootcert', [CompletionResultType]::ParameterName, 'CA certificate (PEM) to trust; turns on full certificate and host name checking. For RDS, the regional bundle from AWS')
            [CompletionResult]::new('--provider', '--provider', [CompletionResultType]::ParameterName, 'Log source')
            [CompletionResult]::new('--instance', '--instance', [CompletionResultType]::ParameterName, 'The instance: RDS instance identifier, Azure server name, Cloud SQL instance, Aiven service, or Heroku app')
            [CompletionResult]::new('--region', '--region', [CompletionResultType]::ParameterName, 'AWS region; default from the AWS CLI configuration')
            [CompletionResult]::new('--profile', '--profile', [CompletionResultType]::ParameterName, 'AWS CLI profile')
            [CompletionResult]::new('--project', '--project', [CompletionResultType]::ParameterName, 'Google Cloud project (`gcp-cloudsql`) or Aiven project (`aiven`)')
            [CompletionResult]::new('--resource-group', '--resource-group', [CompletionResultType]::ParameterName, 'Azure resource group (`azure-flexible`)')
            [CompletionResult]::new('--subscription', '--subscription', [CompletionResultType]::ParameterName, 'Azure subscription (`azure-flexible`)')
            [CompletionResult]::new('--log-group', '--log-group', [CompletionResultType]::ParameterName, '`CloudWatch` log group (`aws-cloudwatch`); default `/aws/rds/instance/<instance>/postgresql`')
            [CompletionResult]::new('--bin', '--bin', [CompletionResultType]::ParameterName, 'Path of the vendor CLI executable (default: `aws`, `az`, `gcloud`, `avn` or `heroku`)')
            [CompletionResult]::new('--jobs', '--jobs', [CompletionResultType]::ParameterName, 'Files downloaded at the same time (`aws-rds`; 1 to 16)')
            [CompletionResult]::new('--interval', '--interval', [CompletionResultType]::ParameterName, 'Time between updates (`30s`, `5m`, `1h`)')
            [CompletionResult]::new('--window', '--window', [CompletionResultType]::ParameterName, 'How far back the report looks (`24h`, `7d`)')
            [CompletionResult]::new('--listen', '--listen', [CompletionResultType]::ParameterName, 'Address to serve the dashboard on (loopback only unless `--allow-remote`)')
            [CompletionResult]::new('--state-dir', '--state-dir', [CompletionResultType]::ParameterName, 'Where snapshots, fetched logs and the store are kept')
            [CompletionResult]::new('--keep-snapshots', '--keep-snapshots', [CompletionResultType]::ParameterName, 'Snapshots to keep (288 is 24 hours at 5 minutes)')
            [CompletionResult]::new('--prefix', '--prefix', [CompletionResultType]::ParameterName, 'The server''s `log_line_prefix`, if the logs are stderr and it cannot be inferred')
            [CompletionResult]::new('--out', '--out', [CompletionResultType]::ParameterName, 'Also write the HTML here after every update')
            [CompletionResult]::new('--allow-remote', '--allow-remote', [CompletionResultType]::ParameterName, 'Allow `--listen` on an address other machines can reach. The page holds statement text')
            [CompletionResult]::new('--once', '--once', [CompletionResultType]::ParameterName, 'Run one cycle and exit instead of looping and serving')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            break
        }
        'pgloupe;help' {
            [CompletionResult]::new('stats', 'stats', [CompletionResultType]::ParameterValue, 'Work with `pg_stat_statements` snapshots')
            [CompletionResult]::new('logs', 'logs', [CompletionResultType]::ParameterValue, 'Work with Postgres server logs')
            [CompletionResult]::new('report', 'report', [CompletionResultType]::ParameterValue, 'One report from logs and, optionally, two `pg_stat_statements` snapshots: HTML, JSON, text or CSV, with the two joined per statement')
            [CompletionResult]::new('snap', 'snap', [CompletionResultType]::ParameterValue, 'Take a `pg_stat_statements` snapshot from a live server (read-only; never resets anything)')
            [CompletionResult]::new('doctor', 'doctor', [CompletionResultType]::ParameterValue, 'Check what the server and the role lack for a useful report, and how to fix it')
            [CompletionResult]::new('export', 'export', [CompletionResultType]::ParameterValue, 'Check the server, take a snapshot and fetch logs into one directory; optionally the report')
            [CompletionResult]::new('watch', 'watch', [CompletionResultType]::ParameterValue, 'Keep a report fresh from a live server and serve it locally')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'pgloupe;help;stats' {
            [CompletionResult]::new('diff', 'diff', [CompletionResultType]::ParameterValue, 'Compare two snapshot CSV exports and rank the queries that changed')
            break
        }
        'pgloupe;help;stats;diff' {
            break
        }
        'pgloupe;help;logs' {
            [CompletionResult]::new('dump', 'dump', [CompletionResultType]::ParameterValue, 'Decode a log (csvlog, jsonlog or stderr; compression is detected) and print each record as JSON')
            [CompletionResult]::new('analyze', 'analyze', [CompletionResultType]::ParameterValue, 'Summarize one or more logs: statements with percentiles, errors, locks, temp files, connections, checkpoints and autovacuum')
            [CompletionResult]::new('ingest', 'ingest', [CompletionResultType]::ParameterValue, 'Add logs to a store; files already ingested are skipped and grown ones replace their old version')
            [CompletionResult]::new('report', 'report', [CompletionResultType]::ParameterValue, 'Report on a time window of a store')
            [CompletionResult]::new('fetch', 'fetch', [CompletionResultType]::ParameterValue, 'Download a time range of log files from a managed service or server (resumable)')
            break
        }
        'pgloupe;help;logs;dump' {
            break
        }
        'pgloupe;help;logs;analyze' {
            break
        }
        'pgloupe;help;logs;ingest' {
            break
        }
        'pgloupe;help;logs;report' {
            break
        }
        'pgloupe;help;logs;fetch' {
            break
        }
        'pgloupe;help;report' {
            break
        }
        'pgloupe;help;snap' {
            break
        }
        'pgloupe;help;doctor' {
            break
        }
        'pgloupe;help;export' {
            break
        }
        'pgloupe;help;watch' {
            break
        }
        'pgloupe;help;help' {
            break
        }
    })

    $completions.Where{ $_.CompletionText -like "$wordToComplete*" } |
        Sort-Object -Property ListItemText
}
