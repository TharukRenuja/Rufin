#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

mkdir -p "$repo_root/target/tmp"
export TMPDIR="${TMPDIR:-$repo_root/target/tmp}"

cargo run --locked -p xtask -- flatpak check-icon-assertions "$@"
