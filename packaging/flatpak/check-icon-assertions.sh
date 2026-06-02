#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
icon_root="$root/data/icons/hicolor"
manifests=(
  "$root/packaging/flatpak/io.github.screwys.Rufin.json"
  "$root/packaging/flatpak/io.github.screwys.Rufin.flathub.json"
)

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required to check Flatpak icon assertions" >&2
  exit 1
fi

mapfile -t icon_paths < <(
  cd "$icon_root"
  find . -type f -printf '%P\n' | sort
)

if [[ "${#icon_paths[@]}" -eq 0 ]]; then
  echo "no icons found in $icon_root" >&2
  exit 1
fi

status=0
for manifest in "${manifests[@]}"; do
  if [[ ! -f "$manifest" ]]; then
    echo "missing Flatpak manifest: $manifest" >&2
    status=1
    continue
  fi

  mapfile -t build_commands < <(
    jq -r '.modules[] | select(.name == "rufin") | .["build-commands"][]' "$manifest"
  )

  for icon_path in "${icon_paths[@]}"; do
    assertion="test -f /app/share/icons/hicolor/$icon_path"
    if ! printf '%s\n' "${build_commands[@]}" | grep -Fxq "$assertion"; then
      echo "$manifest missing icon assertion: $assertion" >&2
      status=1
    fi
  done
done

exit "$status"
