#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
flake_file="$root/flake.nix"
fake_hash="sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
check=0

if [[ "${1:-}" == "--check" ]]; then
  check=1
  shift
fi

if [[ "$#" -ne 0 ]]; then
  echo "usage: $0 [--check]" >&2
  exit 2
fi

current_hash="$(
  sed -n 's/^[[:space:]]*cargoHash = "\(sha256-[^"]*\)";$/\1/p' "$flake_file" |
    head -n 1
)"

if [[ -z "$current_hash" ]]; then
  echo "could not find cargoHash in flake.nix" >&2
  exit 1
fi

perl -0pi -e \
  's/cargoHash = "sha256-[^"]+";/cargoHash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";/' \
  "$flake_file"

tmp="$(mktemp)"
cleanup() {
  rm -f "$tmp"
}
trap cleanup EXIT

set +e
(
  cd "$root"
  nix --accept-flake-config --extra-experimental-features "nix-command flakes" \
    build .#rufin --no-link --print-build-logs
) >"$tmp" 2>&1
status=$?
set -e

new_hash="$(
  sed -n 's/^[[:space:]]*got:[[:space:]]*\(sha256-[A-Za-z0-9+/=]*\)$/\1/p' "$tmp" |
    tail -n 1
)"

if [[ -z "$new_hash" ]]; then
  perl -0pi -e "s|cargoHash = \"\Q$fake_hash\E\";|cargoHash = \"$current_hash\";|" "$flake_file"
  cat "$tmp" >&2
  echo "could not determine cargoHash" >&2
  exit "$status"
fi

if [[ "$new_hash" == "$current_hash" ]]; then
  perl -0pi -e "s|cargoHash = \"\Q$fake_hash\E\";|cargoHash = \"$current_hash\";|" "$flake_file"
  echo "cargoHash is already up to date: $new_hash"
elif [[ "$check" == "1" ]]; then
  perl -0pi -e "s|cargoHash = \"\Q$fake_hash\E\";|cargoHash = \"$current_hash\";|" "$flake_file"
  echo "cargoHash is stale: $current_hash -> $new_hash" >&2
  exit 1
else
  perl -0pi -e "s|cargoHash = \"\Q$fake_hash\E\";|cargoHash = \"$new_hash\";|" "$flake_file"
  echo "updated cargoHash: $current_hash -> $new_hash"
fi
