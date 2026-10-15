#!/bin/sh
# Line coverage of the workspace. Needs: cargo install cargo-llvm-cov
set -eu
cd "$(dirname "$0")/.."
cargo llvm-cov --workspace --summary-only "$@"
echo "HTML report: cargo llvm-cov --workspace --html --open"
