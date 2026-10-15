#!/bin/sh
# Runs the ignored corpus tests against every generated Postgres version.
# Generate the corpora first: cargo run -p synth -- generate --pg <14|15|16|17|18>
set -eu
for dir in data/synth/*-42; do
    version=${dir#data/synth/}
    version=${version%-42}
    echo "== Postgres $version"
    PGLOUPE_PG=$version cargo test --release -p pgloupe --tests -- --ignored
done
