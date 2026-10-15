# pgBouncer, other languages, other clouds

These work, but they have been tested on generated and invented input rather than production logs. Treat their output with a little extra care and report what looks wrong.

## pgBouncer logs

pgloupe recognises a pgBouncer log by its shape (or give `--format pgbouncer`). Logins, why sessions ended, and the once-a-minute `stats:` line (transactions, queries, wait time) get their own *Pooler* view: logins, mean client wait, peak transactions per second. A log with a time-zone name other than UTC needs `--log-tz`, and pgloupe says so.

## Servers that log in another language

Portuguese (Brazil), Spanish, French and German are read as if they were English: the language is detected, or give `--lc-messages auto|en|pt_BR|es|fr|de`. The translations come from PostgreSQL's own message catalogs. `--messages-file` adds another language from a table of `locale<TAB>english<TAB>translated`.

Limits: autovacuum lines are not translated, error texts stay in their own language, and reading is a few times slower than English because every message is matched against the translations.

## Other clouds

CloudWatch, Azure, Cloud SQL, Aiven and Heroku are available as `--provider` values. They are tested against recorded payloads, not the real services. Amazon RDS (`aws-rds`) is the first target, and logs read over SQL (`sql-logdir`) are tested against real Postgres servers 14 to 18 in containers.
