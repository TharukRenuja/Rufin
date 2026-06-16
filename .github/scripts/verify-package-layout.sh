#!/usr/bin/env bash
set -euo pipefail

root="${1:?usage: $0 ROOT [PREFIX]}"
prefix="${2:-/usr}"

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
root="${root%/}"
[[ "$root" == "/" ]] && root=""
prefix="/${prefix#/}"
[[ "$prefix" == "/" ]] && prefix=""

pkg_path() {
  printf '%s%s/%s\n' "$root" "$prefix" "$1"
}

require_file() {
  local path="$1"
  [[ -f "$path" ]] || {
    echo "missing file: $path" >&2
    exit 1
  }
}

if [[ -n "$prefix" ]]; then
  require_file "$(pkg_path bin/rufin)"
else
  if [[ -f "$(pkg_path bin/rufin)" ]]; then
    :
  elif [[ -f "$(pkg_path rufin.exe)" ]]; then
    :
  else
    echo "missing executable under $root" >&2
    exit 1
  fi
fi

require_file "$(pkg_path share/applications/io.github.screwys.Rufin.desktop)"
require_file "$(pkg_path share/metainfo/io.github.screwys.Rufin.metainfo.xml)"

while IFS= read -r icon; do
  rel="${icon#"$repo/data/icons/hicolor/"}"
  require_file "$(pkg_path "share/icons/hicolor/$rel")"
done < <(find "$repo/data/icons/hicolor" -type f | sort)

shopt -s nullglob
for po_file in "$repo"/locales/*.po; do
  lang="$(basename "$po_file" .po)"
  require_file "$(pkg_path "share/locale/$lang/LC_MESSAGES/rufin.mo")"
done
