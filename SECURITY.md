# Security policy

## Reporting a vulnerability

Please report it privately through GitHub's
[private vulnerability reporting](https://github.com/vtrbraga/pgloupe/security/advisories/new)
rather than a public issue. Expect an answer within a week.

## What matters here

pgloupe reads logs and statistics that contain query text, and some commands connect to servers
and cloud tools. These are the areas to look at:

- **Reports hold statement text.** `--anonymize` rewrites names and literals, but a report is
  still sensitive. `watch` serves on loopback by default and refuses other addresses without
  `--allow-remote`; it answers only localhost host names.
- **Connections are read-only.** `snap`, `doctor`, `export` and `watch` set the session read-only
  and never reset statistics. Passwords are never printed and are taken from the DSN, an
  environment variable or `~/.pgpass`.
- **Vendor CLIs run without a shell.** Arguments are passed as lists.
- **Untrusted input.** Logs and archives are parsed, never executed. A crash or hang on a crafted
  log is a bug worth reporting.

## Supported versions

The latest release and `main`.
