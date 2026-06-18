#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 0 ]]; then
  echo "Usage: $0" >&2
  exit 2
fi

cargo clippy --workspace --lib --bins --locked -- \
  -D warnings \
  -D clippy::expect_used \
  -D clippy::panic
cargo clippy --workspace --tests --benches --examples --locked -- -D warnings
cargo clippy -p domain --lib --all-features --locked -- \
  -D clippy::indexing_slicing
