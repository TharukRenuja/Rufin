#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
tmp="$(mktemp -d)"
cleanup() {
  rm -rf "$tmp"
}
trap cleanup EXIT

log="$tmp/gh.log"
counter="$tmp/pr-view-count"

cat > "$tmp/gh" <<'GH'
#!/usr/bin/env bash
set -euo pipefail

printf '%s\n' "$*" >> "$GH_TEST_LOG"

case "$1 $2" in
  "pr view")
    count="$(cat "$GH_TEST_COUNTER" 2>/dev/null || printf '0')"
    count="$((count + 1))"
    printf '%s\n' "$count" > "$GH_TEST_COUNTER"
    if [[ "$count" -lt 3 ]]; then
      printf '0\n'
    else
      printf '1\n'
    fi
    ;;
  "pr checks")
    count="$(cat "$GH_TEST_COUNTER" 2>/dev/null || printf '0')"
    if [[ "$count" -lt 3 ]]; then
      printf "no checks reported on the branch\n" >&2
      exit 1
    fi
    ;;
  *)
    printf 'unexpected gh command: %s\n' "$*" >&2
    exit 1
    ;;
esac
GH
chmod +x "$tmp/gh"

PATH="$tmp:$PATH" \
GH_TEST_COUNTER="$counter" \
GH_TEST_LOG="$log" \
RUFIN_PR_CHECK_DISCOVERY_ATTEMPTS=3 \
RUFIN_PR_CHECK_DISCOVERY_INTERVAL=0 \
RUFIN_PR_CHECK_WATCH_INTERVAL=1 \
  "$root/.github/scripts/watch-pr-checks.sh" \
  --repo example/project \
  42 \
  "release metadata"

view_count="$(grep -c '^pr view 42 ' "$log")"
if [[ "$view_count" != "3" ]]; then
  printf 'expected 3 pr view attempts, got %s\n' "$view_count" >&2
  cat "$log" >&2
  exit 1
fi

if ! grep -q '^pr checks 42 ' "$log"; then
  printf 'expected pr checks after discovery\n' >&2
  cat "$log" >&2
  exit 1
fi
