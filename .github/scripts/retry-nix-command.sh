#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage: .github/scripts/retry-nix-command.sh COMMAND [ARG...]

Retries Nix commands only when their output matches known transient
network or binary-cache failures.
USAGE
}

if [[ $# -eq 0 ]]; then
  usage
  exit 1
fi

attempts="${RUFIN_NIX_RETRY_ATTEMPTS:-3}"
delay_seconds="${RUFIN_NIX_RETRY_DELAY_SECONDS:-20}"

if [[ ! "$attempts" =~ ^[1-9][0-9]*$ ]]; then
  echo "RUFIN_NIX_RETRY_ATTEMPTS must be a positive integer" >&2
  exit 1
fi

if [[ ! "$delay_seconds" =~ ^[0-9]+$ ]]; then
  echo "RUFIN_NIX_RETRY_DELAY_SECONDS must be a non-negative integer" >&2
  exit 1
fi

log_file="$(mktemp)"
trap 'rm -f "$log_file"' EXIT

is_transient_nix_failure() {
  grep -Eiq \
    'substitutes?.*(failed|unavailable)|HTTP error (408|409|416|425|429|5[0-9][0-9])|download(ing)? .* (failed|timed out)|unable to download|could not download|failed to download|connection (reset|refused|timed out)|network is unreachable|temporary failure|unexpected EOF|transfer closed|curl:|curl error|curl.*(failed|timed out|transfer)|(TLS|SSL).*(failed|error|connect)' \
    "$log_file"
}

attempt=1
while (( attempt <= attempts )); do
  : > "$log_file"
  set +e
  "$@" >"$log_file" 2>&1
  status="$?"
  set -e
  cat "$log_file"

  if [[ "$status" -eq 0 ]]; then
    exit 0
  fi

  if (( attempt == attempts )); then
    exit "$status"
  fi

  if ! is_transient_nix_failure; then
    exit "$status"
  fi

  sleep_for=$((delay_seconds * attempt))
  echo "Nix command hit a likely transient cache or network failure; retrying in ${sleep_for}s..." >&2
  sleep "$sleep_for"
  attempt=$((attempt + 1))
done

exit "$status"
