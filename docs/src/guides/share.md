# Share a report safely

```bash
pgloupe report postgresql.log --anonymize --output html --out shareable.html
```

`--anonymize` rewrites names and literals as records are read, before anything is counted, so every output (HTML, JSON, CSV, text, a store) is covered.

- Databases, users, applications and hosts become stable pseudonyms; query literals and error values are replaced; free-text messages the tool does not recognise are redacted.
- Pseudonyms are keyed. Give your own key with `--anonymize-key` or `PGLOUPE_ANONYMIZE_KEY`: the same key gives the same pseudonyms in two reports, so they can be compared with each other and with nobody else's. Keep the key private; anyone with it can test guesses against the pseudonyms.
- A store built with `--anonymize` stays anonymous; its report does not print the filter it was built with.
- Bind parameters are never kept unless you pass `--params`, and `--params` and `--anonymize` should not be combined for a report you intend to share.

Check a shared report before you send it: anonymizing removes what pgloupe can recognise, not everything a person could infer from the shape of a workload.
