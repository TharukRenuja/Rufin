#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage: .github/scripts/update-flathub-manifest.sh [--manifest PATH] TAG

Updates the Flathub submission manifest to build TAG from the exact commit
targeted by that tag. TAG may be vX.Y.Z or X.Y.Z.
USAGE
}

manifest="packaging/flatpak/io.github.screwys.Rufin.flathub.json"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --manifest)
      manifest="${2:-}"
      if [[ -z "$manifest" ]]; then
        echo "--manifest requires a path" >&2
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

tag="${1:-}"
if [[ -z "$tag" ]]; then
  usage
  exit 1
fi

if [[ "$tag" != v* ]]; then
  tag="v$tag"
fi

if [[ ! "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?$ ]]; then
  echo "tag must look like vX.Y.Z" >&2
  exit 1
fi

if [[ ! -f "$manifest" ]]; then
  echo "manifest does not exist: $manifest" >&2
  exit 1
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required to update $manifest" >&2
  exit 1
fi

if ! git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
  echo "tag does not exist: $tag" >&2
  exit 1
fi

plain_version="${tag#v}"
commit="$(git rev-list -n 1 "$tag")"
cargo_version="$(git show "$tag:crates/rufin-app/Cargo.toml" |
  sed -n 's/^version = "\(.*\)"/\1/p' |
  head -n 1)"
metainfo_version="$(git show "$tag:data/io.github.screwys.Rufin.metainfo.xml" |
  sed -n 's/.*<release version="\([^"]*\)".*/\1/p' |
  head -n 1)"

if [[ "$cargo_version" != "$plain_version" ]]; then
  echo "tag $tag has Cargo version $cargo_version, expected $plain_version" >&2
  exit 1
fi

if [[ "$metainfo_version" != "$plain_version" ]]; then
  echo "tag $tag has MetaInfo release $metainfo_version, expected $plain_version" >&2
  exit 1
fi

tmp="$(mktemp)"
cleanup() {
  rm -f "$tmp"
}
trap cleanup EXIT

jq \
  --arg tag "$tag" \
  --arg commit "$commit" \
  --arg url "https://github.com/screwys/Rufin.git" \
  '
  if any(.modules[]?; .name == "rufin") | not then
    error("missing rufin module")
  else
    .modules |= map(
      if .name == "rufin" then
        .sources[0] = ((.sources[0] // {}) + {
          "type": "git",
          "url": $url,
          "tag": $tag,
          "commit": $commit
        })
      else
        .
      end
    )
  end
  ' "$manifest" > "$tmp"

mv "$tmp" "$manifest"
jq empty "$manifest"

printf 'Updated %s to %s (%s)\n' "$manifest" "$tag" "$commit"
