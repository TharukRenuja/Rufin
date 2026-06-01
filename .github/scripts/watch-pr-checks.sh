#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage: .github/scripts/watch-pr-checks.sh [--repo OWNER/REPO] PR_NUMBER PHASE

Waits until GitHub reports check runs for a pull request, then watches them.
USAGE
}

repo=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo)
      repo="${2:-}"
      if [[ -z "$repo" ]]; then
        echo "--repo requires OWNER/REPO" >&2
        exit 1
      fi
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    --)
      shift
      break
      ;;
    -*)
      echo "unknown option: $1" >&2
      usage
      exit 1
      ;;
    *)
      break
      ;;
  esac
done

pr_number="${1:-}"
phase="${2:-}"

if [[ -z "$pr_number" || -z "$phase" ]]; then
  usage
  exit 1
fi

repo_args=()
if [[ -n "$repo" ]]; then
  repo_args=(--repo "$repo")
fi

attempts="${RUFIN_PR_CHECK_DISCOVERY_ATTEMPTS:-40}"
discovery_interval="${RUFIN_PR_CHECK_DISCOVERY_INTERVAL:-3}"
watch_interval="${RUFIN_PR_CHECK_WATCH_INTERVAL:-15}"

if [[ ! "$attempts" =~ ^[0-9]+$ || "$attempts" -lt 1 ]]; then
  echo "RUFIN_PR_CHECK_DISCOVERY_ATTEMPTS must be a positive integer" >&2
  exit 1
fi

if [[ ! "$discovery_interval" =~ ^[0-9]+$ ]]; then
  echo "RUFIN_PR_CHECK_DISCOVERY_INTERVAL must be a non-negative integer" >&2
  exit 1
fi

if [[ ! "$watch_interval" =~ ^[0-9]+$ || "$watch_interval" -lt 1 ]]; then
  echo "RUFIN_PR_CHECK_WATCH_INTERVAL must be a positive integer" >&2
  exit 1
fi

printf '\nWaiting for PR #%s Checks (%s)...\n' "$pr_number" "$phase"
for ((attempt = 1; attempt <= attempts; attempt++)); do
  check_count="$(
    gh pr view "$pr_number" "${repo_args[@]}" \
      --json statusCheckRollup \
      --jq '[.statusCheckRollup[]?] | length'
  )"

  if [[ "$check_count" =~ ^[0-9]+$ && "$check_count" -gt 0 ]]; then
    gh pr checks "$pr_number" "${repo_args[@]}" \
      --watch \
      --fail-fast \
      --interval "$watch_interval"
    exit 0
  fi

  if [[ "$attempt" -lt "$attempts" ]]; then
    sleep "$discovery_interval"
  fi
done

echo "no checks reported for PR #$pr_number after waiting" >&2
exit 1
