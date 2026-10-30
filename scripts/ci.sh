#!/usr/bin/env bash
# Local CI: formatting, lints, tests. Run from anywhere in the repo.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> cargo fmt --check"
cargo fmt --all --check

echo "==> cargo clippy"
cargo clippy --workspace --all-targets -- -D warnings

echo "==> cargo test"
cargo test --workspace

echo "==> matching eval (fails on any wrong plan)"
cargo run -q -p sancho-eval -- eval/requests.toml

echo "CI passed."
