#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
lock_file="$root/Cargo.lock"
sources_file="$root/packaging/flatpak/cargo-sources.json"
check=0

usage() {
  cat >&2 <<'USAGE'
Usage: packaging/flatpak/update-cargo-sources.sh [--check]

Regenerates packaging/flatpak/cargo-sources.json from Cargo.lock.
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --check)
      check=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown option: $1" >&2
      usage
      exit 1
      ;;
  esac
done

tmp="$(mktemp)"
cleanup() {
  rm -f "$tmp"
}
trap cleanup EXIT

awk '
function quoted_value(line, value) {
    value = line
    sub(/^[^=]+ = "/, "", value)
    sub(/"$/, "", value)
    return value
}

function emit_crate(name, version, checksum, dest) {
    dest = "cargo/vendor/" name "-" version
    print "    {"
    print "        \"type\": \"archive\","
    print "        \"archive-type\": \"tar-gzip\","
    print "        \"url\": \"https://static.crates.io/crates/" name "/" name "-" version ".crate\","
    print "        \"sha256\": \"" checksum "\","
    print "        \"dest\": \"" dest "\""
    print "    },"
    print "    {"
    print "        \"type\": \"inline\","
    print "        \"contents\": \"{\\\"package\\\": \\\"" checksum "\\\", \\\"files\\\": {}}\","
    print "        \"dest\": \"" dest "\","
    print "        \"dest-filename\": \".cargo-checksum.json\""
    print "    },"
    emitted = 1
}

function flush_package() {
    if (source == "registry+https://github.com/rust-lang/crates.io-index") {
        if (checksum == "") {
            print "missing checksum for " name " " version > "/dev/stderr"
            exit 1
        }
        key = name "\t" version "\t" checksum
        if (!(key in seen)) {
            seen[key] = 1
            emit_crate(name, version, checksum)
        }
    }
    name = ""
    version = ""
    source = ""
    checksum = ""
}

BEGIN {
    print "["
}

/^\[\[package\]\]/ {
    if (in_package) {
        flush_package()
    }
    in_package = 1
    next
}

in_package && /^name = "/ {
    name = quoted_value($0)
    next
}

in_package && /^version = "/ {
    version = quoted_value($0)
    next
}

in_package && /^source = "/ {
    source = quoted_value($0)
    next
}

in_package && /^checksum = "/ {
    checksum = quoted_value($0)
    next
}

END {
    if (in_package) {
        flush_package()
    }
    print "    {"
    print "        \"type\": \"inline\","
    print "        \"contents\": \"[source.vendored-sources]\\ndirectory = \\\"cargo/vendor\\\"\\n\\n[source.crates-io]\\nreplace-with = \\\"vendored-sources\\\"\\n\","
    print "        \"dest\": \"cargo\","
    print "        \"dest-filename\": \"config\""
    print "    }"
    print "]"
}
' "$lock_file" > "$tmp"

if [[ "$check" == "1" ]]; then
  if ! cmp -s "$tmp" "$sources_file"; then
    echo "packaging/flatpak/cargo-sources.json is stale; run packaging/flatpak/update-cargo-sources.sh" >&2
    exit 1
  fi
  exit 0
fi

mv "$tmp" "$sources_file"
trap - EXIT
