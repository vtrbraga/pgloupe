# Print an optspec for argparse to handle cmd's options that are independent of any subcommand.
function __fish_pgloupe_global_optspecs
    string join \n h/help V/version
end

function __fish_pgloupe_needs_command
    # Figure out if the current invocation already has a command.
    set -l cmd (commandline -opc)
    set -e cmd[1]
    argparse -s (__fish_pgloupe_global_optspecs) -- $cmd 2>/dev/null
    or return
    if set -q argv[1]
        # Also print the command, so this can be used to figure out what it is.
        echo $argv[1]
        return 1
    end
    return 0
end

function __fish_pgloupe_using_subcommand
    set -l cmd (__fish_pgloupe_needs_command)
    test -z "$cmd"
    and return 1
    contains -- $cmd[1] $argv
end

complete -c pgloupe -n "__fish_pgloupe_needs_command" -s h -l help -d 'Print help'
complete -c pgloupe -n "__fish_pgloupe_needs_command" -s V -l version -d 'Print version'
complete -c pgloupe -n "__fish_pgloupe_needs_command" -f -a "stats" -d 'Work with `pg_stat_statements` snapshots'
complete -c pgloupe -n "__fish_pgloupe_needs_command" -f -a "logs" -d 'Work with Postgres server logs'
complete -c pgloupe -n "__fish_pgloupe_needs_command" -f -a "report" -d 'One report from logs and, optionally, two `pg_stat_statements` snapshots: HTML, JSON, text or CSV, with the two joined per statement'
complete -c pgloupe -n "__fish_pgloupe_needs_command" -f -a "snap" -d 'Take a `pg_stat_statements` snapshot from a live server (read-only; never resets anything)'
complete -c pgloupe -n "__fish_pgloupe_needs_command" -f -a "doctor" -d 'Check what the server and the role lack for a useful report, and how to fix it'
complete -c pgloupe -n "__fish_pgloupe_needs_command" -f -a "export" -d 'Check the server, take a snapshot and fetch logs into one directory; optionally the report'
complete -c pgloupe -n "__fish_pgloupe_needs_command" -f -a "watch" -d 'Keep a report fresh from a live server and serve it locally'
complete -c pgloupe -n "__fish_pgloupe_needs_command" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand stats; and not __fish_seen_subcommand_from diff help" -s h -l help -d 'Print help'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand stats; and not __fish_seen_subcommand_from diff help" -f -a "diff" -d 'Compare two snapshot CSV exports and rank the queries that changed'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand stats; and not __fish_seen_subcommand_from diff help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand stats; and __fish_seen_subcommand_from diff" -l before-info -d '`pg_stat_statements_info` CSV taken with the older snapshot' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand stats; and __fish_seen_subcommand_from diff" -l after-info -d '`pg_stat_statements_info` CSV taken with the newer snapshot' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand stats; and __fish_seen_subcommand_from diff" -l by -d 'Metric that orders the table' -r -f -a "total\t'Δ total execution time'
calls\t'Δ calls'
mean\t'Mean execution time inside the window'"
complete -c pgloupe -n "__fish_pgloupe_using_subcommand stats; and __fish_seen_subcommand_from diff" -l top -d 'Rows to show (`all` for every statement)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand stats; and __fish_seen_subcommand_from diff" -l min-calls -d 'With `--by mean`, skip queries with fewer calls than this in the window' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand stats; and __fish_seen_subcommand_from diff" -l format -d 'Output format' -r -f -a "text\t''
json\t''"
complete -c pgloupe -n "__fish_pgloupe_using_subcommand stats; and __fish_seen_subcommand_from diff" -l wide -d 'Show full query text in the table'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand stats; and __fish_seen_subcommand_from diff" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand stats; and __fish_seen_subcommand_from help" -f -a "diff" -d 'Compare two snapshot CSV exports and rank the queries that changed'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand stats; and __fish_seen_subcommand_from help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and not __fish_seen_subcommand_from dump analyze ingest report fetch help" -s h -l help -d 'Print help'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and not __fish_seen_subcommand_from dump analyze ingest report fetch help" -f -a "dump" -d 'Decode a log (csvlog, jsonlog or stderr; compression is detected) and print each record as JSON'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and not __fish_seen_subcommand_from dump analyze ingest report fetch help" -f -a "analyze" -d 'Summarize one or more logs: statements with percentiles, errors, locks, temp files, connections, checkpoints and autovacuum'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and not __fish_seen_subcommand_from dump analyze ingest report fetch help" -f -a "ingest" -d 'Add logs to a store; files already ingested are skipped and grown ones replace their old version'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and not __fish_seen_subcommand_from dump analyze ingest report fetch help" -f -a "report" -d 'Report on a time window of a store'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and not __fish_seen_subcommand_from dump analyze ingest report fetch help" -f -a "fetch" -d 'Download a time range of log files from a managed service or server (resumable)'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and not __fish_seen_subcommand_from dump analyze ingest report fetch help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from dump" -l format -d 'Log format; `auto` looks at the first line' -r -f -a "auto\t''
csv\t''
json\t''
stderr\t''
pgbouncer\t'The pgBouncer log (experimental)'"
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from dump" -l prefix -d 'The server\'s `log_line_prefix`; required for stderr logs' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from dump" -l log-tz -d 'Offset for timezone abbreviations the log does not spell out, e.g. `-03` or `+05:30`' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from dump" -l limit -d 'Stop after this many records' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from dump" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l format -d 'Log format; `auto` looks at each file\'s first line' -r -f -a "auto\t''
csv\t''
json\t''
stderr\t''
pgbouncer\t'The pgBouncer log (experimental)'"
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l prefix -d 'The server\'s `log_line_prefix`; inferred when omitted' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l log-tz -d 'Offset for timezone abbreviations the log does not spell out, e.g. `-03` or `+05:30`' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l by -d 'Metric that orders the statement table' -r -f -a "total\t'Total execution time'
calls\t'Number of calls'
p95\t'95th percentile of the duration'
max\t'Slowest single call'"
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l top -d 'Statements to list (`all` for every statement)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l bucket -d 'Timeline resolution: `30s`, `1m`, `1h`' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l max-fingerprints -d 'Distinct statements kept before the rest fold into one overflow row' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l threads -d 'Worker threads (default: all cores)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l output -d 'Report format' -r -f -a "text\t''
json\t''"
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l slowest -d 'Slowest single executions to keep and list (default 20)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l database -d 'Only records of these databases (repeat or comma-separate)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l user -d 'Only records of these users' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l application -d 'Only records of these applications (`application_name`)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l host -d 'Only records from these client hosts' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l exclude-database -d 'Drop records of these databases' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l exclude-user -d 'Drop records of these users' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l exclude-application -d 'Drop records of these applications' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l exclude-host -d 'Drop records from these client hosts' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l include-query -d 'Only records whose message or statement matches this regular expression' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l exclude-query -d 'Drop records whose message or statement matches this regular expression' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l pid -d 'Only records of these backend process ids' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l exclude-message -d 'Drop records whose message matches this regular expression (noise such as health checks)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l between -d 'Only records inside this time of day, as `HH:MM-HH:MM` (wraps past midnight)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l exclude-hours -d 'Drop records in these hours of the day, as `5-7` (hours 5 to 7 inclusive); repeatable' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l weekdays -d 'Only records on these days: `mon-fri`, `sat,sun`' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l tz -d 'Zone for `--between`, `--exclude-hours` and `--weekdays`: an IANA name or `UTC` (default)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l anonymize-key -d 'Secret that decides the pseudonyms (default: none, so common names are guessable)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l lc-messages -d 'The language of the server\'s messages (`lc_messages`): `auto` (default) looks at the log, `en` reads English, or one of `pt_BR`, `es`, `fr`, `de`' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l messages-file -d 'A translation table for another language: lines of `locale<TAB>english<TAB>translated`, as in the built-in one' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l params -d 'Keep the bind parameters of those executions; they can hold personal data'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -l anonymize -d 'Make the report safe to share: users, databases, applications and hosts become stable pseudonyms, and literals, emails and addresses are removed from every text'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from analyze" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -l store -d 'The store file; created when missing' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -l format -d 'Log format; `auto` looks at each file\'s first line' -r -f -a "auto\t''
csv\t''
json\t''
stderr\t''
pgbouncer\t'The pgBouncer log (experimental)'"
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -l prefix -d 'The server\'s `log_line_prefix`; inferred when omitted' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -l log-tz -d 'Offset for timezone abbreviations the log does not spell out, e.g. `-03` or `+05:30`' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -l max-fingerprints -d 'Distinct statements kept per hour before the rest fold into one overflow row' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -l threads -d 'Worker threads (default: all cores)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -l slowest -d 'Slowest single executions to keep and list (default 20)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -l database -d 'Only records of these databases (repeat or comma-separate)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -l user -d 'Only records of these users' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -l application -d 'Only records of these applications (`application_name`)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -l host -d 'Only records from these client hosts' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -l exclude-database -d 'Drop records of these databases' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -l exclude-user -d 'Drop records of these users' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -l exclude-application -d 'Drop records of these applications' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -l exclude-host -d 'Drop records from these client hosts' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -l include-query -d 'Only records whose message or statement matches this regular expression' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -l exclude-query -d 'Drop records whose message or statement matches this regular expression' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -l pid -d 'Only records of these backend process ids' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -l exclude-message -d 'Drop records whose message matches this regular expression (noise such as health checks)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -l between -d 'Only records inside this time of day, as `HH:MM-HH:MM` (wraps past midnight)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -l exclude-hours -d 'Drop records in these hours of the day, as `5-7` (hours 5 to 7 inclusive); repeatable' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -l weekdays -d 'Only records on these days: `mon-fri`, `sat,sun`' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -l tz -d 'Zone for `--between`, `--exclude-hours` and `--weekdays`: an IANA name or `UTC` (default)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -l anonymize-key -d 'Secret that decides the pseudonyms (default: none, so common names are guessable)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -l lc-messages -d 'The language of the server\'s messages (`lc_messages`): `auto` (default) looks at the log, `en` reads English, or one of `pt_BR`, `es`, `fr`, `de`' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -l messages-file -d 'A translation table for another language: lines of `locale<TAB>english<TAB>translated`, as in the built-in one' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -l force -d 'Read and store files even when their content is already in the store'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -l params -d 'Keep the bind parameters of those executions; they can hold personal data'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -l anonymize -d 'Make the report safe to share: users, databases, applications and hosts become stable pseudonyms, and literals, emails and addresses are removed from every text'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from ingest" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from report" -l store -d 'The store file written by `logs ingest`' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from report" -l slowest -d 'Slowest executions to list, at most what the ingest kept (20 unless it was told more)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from report" -l since -d 'Start of the window (RFC 3339 or `YYYY-MM-DD`, UTC); rounded down to the hour' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from report" -l until -d 'End of the window; rounded up to the hour' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from report" -l by -d 'Metric that orders the statement table' -r -f -a "total\t'Total execution time'
calls\t'Number of calls'
p95\t'95th percentile of the duration'
max\t'Slowest single call'"
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from report" -l top -d 'Statements to list (`all` for every statement)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from report" -l bucket -d 'Timeline resolution: a multiple of one minute' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from report" -l output -d 'Report format' -r -f -a "text\t''
json\t''"
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from report" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from fetch" -l provider -d 'Log source' -r -f -a "aws-rds\t'Amazon RDS and Aurora PostgreSQL, through the `aws` CLI'
aws-cloudwatch\t'RDS/Aurora logs exported to `CloudWatch` Logs (EXPERIMENTAL)'
azure-flexible\t'Azure Database for PostgreSQL Flexible Server, through `az` (EXPERIMENTAL)'
gcp-cloudsql\t'Google Cloud SQL for PostgreSQL, through `gcloud` (EXPERIMENTAL)'
aiven\t'Aiven for PostgreSQL, through `avn` (EXPERIMENTAL)'
heroku\t'Heroku Postgres, through `heroku`: recent lines only (EXPERIMENTAL)'
sql\t'The server\'s own log directory over SQL (EXPERIMENTAL)'"
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from fetch" -l instance -d 'The instance: RDS instance identifier, Azure server name, Cloud SQL instance, Aiven service, or Heroku app' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from fetch" -l region -d 'AWS region; default from the AWS CLI configuration' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from fetch" -l profile -d 'AWS CLI profile' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from fetch" -l project -d 'Google Cloud project (`gcp-cloudsql`) or Aiven project (`aiven`)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from fetch" -l resource-group -d 'Azure resource group (`azure-flexible`)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from fetch" -l subscription -d 'Azure subscription (`azure-flexible`)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from fetch" -l log-group -d '`CloudWatch` log group (`aws-cloudwatch`); default `/aws/rds/instance/<instance>/postgresql`' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from fetch" -l bin -d 'Path of the vendor CLI executable (default: `aws`, `az`, `gcloud`, `avn` or `heroku`)' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from fetch" -l jobs -d 'Files downloaded at the same time (`aws-rds`; 1 to 16)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from fetch" -l conn -d 'Connection for `--provider sql`' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from fetch" -l password-env -d 'Environment variable that holds the password (default `PGPASSWORD`)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from fetch" -l sslrootcert -d 'CA certificate (PEM) to trust' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from fetch" -l since -d 'Start: `30m`, `24h`, `7d`, `2w` back from now, or an RFC 3339 time or `YYYY-MM-DD` (UTC)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from fetch" -l out -d 'Directory the files go to (created if missing); a rerun continues where this stopped' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from fetch" -l quiet -d 'Do not print a line for each finished file'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from fetch" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from help" -f -a "dump" -d 'Decode a log (csvlog, jsonlog or stderr; compression is detected) and print each record as JSON'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from help" -f -a "analyze" -d 'Summarize one or more logs: statements with percentiles, errors, locks, temp files, connections, checkpoints and autovacuum'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from help" -f -a "ingest" -d 'Add logs to a store; files already ingested are skipped and grown ones replace their old version'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from help" -f -a "report" -d 'Report on a time window of a store'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from help" -f -a "fetch" -d 'Download a time range of log files from a managed service or server (resumable)'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand logs; and __fish_seen_subcommand_from help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l store -d 'A store written by `logs ingest`, instead of log files' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l since -d 'With `--store`: start of the log window (RFC 3339 or `YYYY-MM-DD`, UTC). Defaults to the snapshot interval; rounded down to the hour' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l until -d 'With `--store`: end of the log window; rounded up to the hour' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l before -d 'Older `pg_stat_statements` snapshot (CSV export)' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l after -d 'Newer snapshot' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l before-info -d '`pg_stat_statements_info` CSV taken with the older snapshot' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l after-info -d '`pg_stat_statements_info` CSV taken with the newer snapshot' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l before-db -d '`pg_stat_database` CSV taken with the older snapshot; found beside it when it is named `pg_stat_database_<time>.csv`' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l after-db -d '`pg_stat_database` CSV taken with the newer snapshot' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l format -d 'Log format of `FILES`; `auto` looks at each file\'s first line' -r -f -a "auto\t''
csv\t''
json\t''
stderr\t''
pgbouncer\t'The pgBouncer log (experimental)'"
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l prefix -d 'The server\'s `log_line_prefix` for `FILES`; inferred when omitted' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l log-tz -d 'Offset for timezone abbreviations the log does not spell out, e.g. `-03` or `+05:30`' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l max-fingerprints -d 'Distinct statements kept before the rest fold into one overflow row (`FILES`)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l threads -d 'Worker threads for `FILES` (default: all cores)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l by -d 'Metric that orders the statement table' -r -f -a "total\t'Total execution time'
calls\t'Number of calls'
p95\t'95th percentile of the duration'
max\t'Slowest single call'"
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l top -d 'Statements to list (`all` for every statement)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l bucket -d 'Timeline resolution: `30s`, `1m`, `1h` (a multiple of one minute with `--store`)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l output -d 'Report format' -r -f -a "text\t''
json\t''
html\t'One self-contained HTML file'
csv\t'The statement table, one row per statement'"
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l baseline -d 'Compare with an earlier period of the store: `previous` (the window just before this one, as long), or give `--baseline-since` and `--baseline-until`' -r -f -a "previous\t'The window just before this one, as long'"
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l baseline-since -d 'Start of the baseline window (needs `--baseline-until`)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l baseline-until -d 'End of the baseline window' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l settings -d '`pg_settings` as CSV (`name,setting`, the export of `SELECT * FROM pg_settings`): the report then says exactly which settings limit it. `snap` writes one next to each snapshot, and it is picked up from beside `--after` when this is not given' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l vendor -d 'Where the server runs, to word advice about settings (`rds`; detected when it can be)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l display-tz -d 'Show times in this IANA zone in the HTML dashboard (default UTC); JSON and text stay UTC' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l out -d 'Write the report here instead of to standard output. With several `--output` formats this is a path prefix: `--output html,json --out report` writes `report.html` and `report.json`' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l slowest -d 'Slowest single executions to keep and list (default 20)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l database -d 'Only records of these databases (repeat or comma-separate)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l user -d 'Only records of these users' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l application -d 'Only records of these applications (`application_name`)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l host -d 'Only records from these client hosts' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l exclude-database -d 'Drop records of these databases' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l exclude-user -d 'Drop records of these users' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l exclude-application -d 'Drop records of these applications' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l exclude-host -d 'Drop records from these client hosts' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l include-query -d 'Only records whose message or statement matches this regular expression' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l exclude-query -d 'Drop records whose message or statement matches this regular expression' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l pid -d 'Only records of these backend process ids' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l exclude-message -d 'Drop records whose message matches this regular expression (noise such as health checks)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l between -d 'Only records inside this time of day, as `HH:MM-HH:MM` (wraps past midnight)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l exclude-hours -d 'Drop records in these hours of the day, as `5-7` (hours 5 to 7 inclusive); repeatable' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l weekdays -d 'Only records on these days: `mon-fri`, `sat,sun`' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l tz -d 'Zone for `--between`, `--exclude-hours` and `--weekdays`: an IANA name or `UTC` (default)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l anonymize-key -d 'Secret that decides the pseudonyms (default: none, so common names are guessable)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l lc-messages -d 'The language of the server\'s messages (`lc_messages`): `auto` (default) looks at the log, `en` reads English, or one of `pt_BR`, `es`, `fr`, `de`' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l messages-file -d 'A translation table for another language: lines of `locale<TAB>english<TAB>translated`, as in the built-in one' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l params -d 'Keep the bind parameters of those executions; they can hold personal data'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -l anonymize -d 'Make the report safe to share: users, databases, applications and hosts become stable pseudonyms, and literals, emails and addresses are removed from every text'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand report" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand snap" -l conn -d 'Connection string: `postgres://user@host:5432/db` or `key=value` pairs. The password comes from the string, `PGPASSWORD` (or `--password-env`), or `~/.pgpass`; prefer those to putting it on the command line' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand snap" -l password-env -d 'Environment variable that holds the password (default `PGPASSWORD`)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand snap" -l sslrootcert -d 'CA certificate (PEM) to trust; turns on full certificate and host name checking. For RDS, the regional bundle from AWS' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand snap" -l out -d 'Directory the snapshot files go to (created if missing)' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand snap" -s h -l help -d 'Print help'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand doctor" -l conn -d 'Connection string: `postgres://user@host:5432/db` or `key=value` pairs. The password comes from the string, `PGPASSWORD` (or `--password-env`), or `~/.pgpass`; prefer those to putting it on the command line' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand doctor" -l password-env -d 'Environment variable that holds the password (default `PGPASSWORD`)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand doctor" -l sslrootcert -d 'CA certificate (PEM) to trust; turns on full certificate and host name checking. For RDS, the regional bundle from AWS' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand doctor" -l vendor -d 'Say where settings live for this vendor' -r -f -a "rds\t'Amazon RDS and Aurora PostgreSQL: settings live in the parameter group'"
complete -c pgloupe -n "__fish_pgloupe_using_subcommand doctor" -l output -d 'Output format' -r -f -a "text\t'Human-readable'
json\t'One JSON document'"
complete -c pgloupe -n "__fish_pgloupe_using_subcommand doctor" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand export" -l conn -d 'Connection string: `postgres://user@host:5432/db` or `key=value` pairs. The password comes from the string, `PGPASSWORD` (or `--password-env`), or `~/.pgpass`; prefer those to putting it on the command line' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand export" -l password-env -d 'Environment variable that holds the password (default `PGPASSWORD`)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand export" -l sslrootcert -d 'CA certificate (PEM) to trust; turns on full certificate and host name checking. For RDS, the regional bundle from AWS' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand export" -l provider -d 'Log source' -r -f -a "aws-rds\t'Amazon RDS and Aurora PostgreSQL, through the `aws` CLI'
aws-cloudwatch\t'RDS/Aurora logs exported to `CloudWatch` Logs (EXPERIMENTAL)'
azure-flexible\t'Azure Database for PostgreSQL Flexible Server, through `az` (EXPERIMENTAL)'
gcp-cloudsql\t'Google Cloud SQL for PostgreSQL, through `gcloud` (EXPERIMENTAL)'
aiven\t'Aiven for PostgreSQL, through `avn` (EXPERIMENTAL)'
heroku\t'Heroku Postgres, through `heroku`: recent lines only (EXPERIMENTAL)'
sql\t'The server\'s own log directory over SQL (EXPERIMENTAL)'"
complete -c pgloupe -n "__fish_pgloupe_using_subcommand export" -l instance -d 'The instance: RDS instance identifier, Azure server name, Cloud SQL instance, Aiven service, or Heroku app' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand export" -l region -d 'AWS region; default from the AWS CLI configuration' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand export" -l profile -d 'AWS CLI profile' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand export" -l project -d 'Google Cloud project (`gcp-cloudsql`) or Aiven project (`aiven`)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand export" -l resource-group -d 'Azure resource group (`azure-flexible`)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand export" -l subscription -d 'Azure subscription (`azure-flexible`)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand export" -l log-group -d '`CloudWatch` log group (`aws-cloudwatch`); default `/aws/rds/instance/<instance>/postgresql`' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand export" -l bin -d 'Path of the vendor CLI executable (default: `aws`, `az`, `gcloud`, `avn` or `heroku`)' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand export" -l jobs -d 'Files downloaded at the same time (`aws-rds`; 1 to 16)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand export" -l since -d 'How far back to fetch logs (see `logs fetch --since`)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand export" -l out -d 'Directory for everything: `snapshots/`, `logs/` and the fetch state' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand export" -l report -d 'Also write the report here (HTML) from the logs and the two newest snapshots' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand export" -l prefix -d 'The server\'s `log_line_prefix`, for stderr logs whose prefix cannot be inferred' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand export" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand watch" -l conn -d 'Connection string: `postgres://user@host:5432/db` or `key=value` pairs. The password comes from the string, `PGPASSWORD` (or `--password-env`), or `~/.pgpass`; prefer those to putting it on the command line' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand watch" -l password-env -d 'Environment variable that holds the password (default `PGPASSWORD`)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand watch" -l sslrootcert -d 'CA certificate (PEM) to trust; turns on full certificate and host name checking. For RDS, the regional bundle from AWS' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand watch" -l provider -d 'Log source' -r -f -a "aws-rds\t'Amazon RDS and Aurora PostgreSQL, through the `aws` CLI'
aws-cloudwatch\t'RDS/Aurora logs exported to `CloudWatch` Logs (EXPERIMENTAL)'
azure-flexible\t'Azure Database for PostgreSQL Flexible Server, through `az` (EXPERIMENTAL)'
gcp-cloudsql\t'Google Cloud SQL for PostgreSQL, through `gcloud` (EXPERIMENTAL)'
aiven\t'Aiven for PostgreSQL, through `avn` (EXPERIMENTAL)'
heroku\t'Heroku Postgres, through `heroku`: recent lines only (EXPERIMENTAL)'
sql\t'The server\'s own log directory over SQL (EXPERIMENTAL)'"
complete -c pgloupe -n "__fish_pgloupe_using_subcommand watch" -l instance -d 'The instance: RDS instance identifier, Azure server name, Cloud SQL instance, Aiven service, or Heroku app' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand watch" -l region -d 'AWS region; default from the AWS CLI configuration' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand watch" -l profile -d 'AWS CLI profile' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand watch" -l project -d 'Google Cloud project (`gcp-cloudsql`) or Aiven project (`aiven`)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand watch" -l resource-group -d 'Azure resource group (`azure-flexible`)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand watch" -l subscription -d 'Azure subscription (`azure-flexible`)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand watch" -l log-group -d '`CloudWatch` log group (`aws-cloudwatch`); default `/aws/rds/instance/<instance>/postgresql`' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand watch" -l bin -d 'Path of the vendor CLI executable (default: `aws`, `az`, `gcloud`, `avn` or `heroku`)' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand watch" -l jobs -d 'Files downloaded at the same time (`aws-rds`; 1 to 16)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand watch" -l interval -d 'Time between updates (`30s`, `5m`, `1h`)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand watch" -l window -d 'How far back the report looks (`24h`, `7d`)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand watch" -l listen -d 'Address to serve the dashboard on (loopback only unless `--allow-remote`)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand watch" -l state-dir -d 'Where snapshots, fetched logs and the store are kept' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand watch" -l keep-snapshots -d 'Snapshots to keep (288 is 24 hours at 5 minutes)' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand watch" -l prefix -d 'The server\'s `log_line_prefix`, if the logs are stderr and it cannot be inferred' -r
complete -c pgloupe -n "__fish_pgloupe_using_subcommand watch" -l out -d 'Also write the HTML here after every update' -r -F
complete -c pgloupe -n "__fish_pgloupe_using_subcommand watch" -l allow-remote -d 'Allow `--listen` on an address other machines can reach. The page holds statement text'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand watch" -l once -d 'Run one cycle and exit instead of looping and serving'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand watch" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand help; and not __fish_seen_subcommand_from stats logs report snap doctor export watch help" -f -a "stats" -d 'Work with `pg_stat_statements` snapshots'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand help; and not __fish_seen_subcommand_from stats logs report snap doctor export watch help" -f -a "logs" -d 'Work with Postgres server logs'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand help; and not __fish_seen_subcommand_from stats logs report snap doctor export watch help" -f -a "report" -d 'One report from logs and, optionally, two `pg_stat_statements` snapshots: HTML, JSON, text or CSV, with the two joined per statement'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand help; and not __fish_seen_subcommand_from stats logs report snap doctor export watch help" -f -a "snap" -d 'Take a `pg_stat_statements` snapshot from a live server (read-only; never resets anything)'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand help; and not __fish_seen_subcommand_from stats logs report snap doctor export watch help" -f -a "doctor" -d 'Check what the server and the role lack for a useful report, and how to fix it'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand help; and not __fish_seen_subcommand_from stats logs report snap doctor export watch help" -f -a "export" -d 'Check the server, take a snapshot and fetch logs into one directory; optionally the report'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand help; and not __fish_seen_subcommand_from stats logs report snap doctor export watch help" -f -a "watch" -d 'Keep a report fresh from a live server and serve it locally'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand help; and not __fish_seen_subcommand_from stats logs report snap doctor export watch help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand help; and __fish_seen_subcommand_from stats" -f -a "diff" -d 'Compare two snapshot CSV exports and rank the queries that changed'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand help; and __fish_seen_subcommand_from logs" -f -a "dump" -d 'Decode a log (csvlog, jsonlog or stderr; compression is detected) and print each record as JSON'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand help; and __fish_seen_subcommand_from logs" -f -a "analyze" -d 'Summarize one or more logs: statements with percentiles, errors, locks, temp files, connections, checkpoints and autovacuum'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand help; and __fish_seen_subcommand_from logs" -f -a "ingest" -d 'Add logs to a store; files already ingested are skipped and grown ones replace their old version'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand help; and __fish_seen_subcommand_from logs" -f -a "report" -d 'Report on a time window of a store'
complete -c pgloupe -n "__fish_pgloupe_using_subcommand help; and __fish_seen_subcommand_from logs" -f -a "fetch" -d 'Download a time range of log files from a managed service or server (resumable)'
