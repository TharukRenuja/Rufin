#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 0 ]]; then
  echo "Usage: $0" >&2
  exit 2
fi

bash packaging/flatpak/check-icon-assertions.sh

if command -v cargo-nextest >/dev/null 2>&1; then
  nextest_args=(--workspace --locked)
  nextest_jobs="${NEXTEST_JOBS:-4}"
  if [[ ! "$nextest_jobs" =~ ^[1-9][0-9]*$ ]]; then
    echo "NEXTEST_JOBS must be a positive integer." >&2
    exit 1
  fi
  nextest_args+=(--test-threads "$nextest_jobs")
  cargo nextest run "${nextest_args[@]}"
  cargo test --workspace --doc --locked
else
  echo "cargo-nextest is unavailable; falling back to cargo test." >&2
  cargo test --workspace --locked
fi
