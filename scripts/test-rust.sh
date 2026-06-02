#!/usr/bin/env bash
set -euo pipefail

bash packaging/flatpak/check-icon-assertions.sh

if command -v cargo-nextest >/dev/null 2>&1; then
  cargo nextest run --workspace --locked
  cargo test --workspace --doc --locked
else
  echo "cargo-nextest is unavailable; falling back to cargo test." >&2
  cargo test --workspace --locked
fi
