#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage: .github/scripts/create-release-tag.sh [--base TAG] [--dry-run] [--push] VERSION SUMMARY

Creates a signed annotated release tag whose message includes commits since the
previous release tag. VERSION may be vX.Y.Z or X.Y.Z.

Examples:
  .github/scripts/create-release-tag.sh --dry-run v0.2.6 "More fixes"
  .github/scripts/create-release-tag.sh --push v0.2.6 "More fixes"
USAGE
}

base_tag=""
dry_run=0
push_tag=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --base)
      base_tag="${2:-}"
      if [[ -z "$base_tag" ]]; then
        echo "--base requires a tag" >&2
        exit 1
      fi
      shift 2
      ;;
    --dry-run)
      dry_run=1
      shift
      ;;
    --push)
      push_tag=1
      shift
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

version="${1:-}"
summary="${2:-}"

if [[ -z "$version" || -z "$summary" ]]; then
  usage
  exit 1
fi

if [[ "$version" != v* ]]; then
  version="v$version"
fi

if [[ ! "$version" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?$ ]]; then
  echo "version must look like vX.Y.Z" >&2
  exit 1
fi

if git rev-parse -q --verify "refs/tags/$version" >/dev/null; then
  echo "tag already exists: $version" >&2
  exit 1
fi

if [[ -z "$base_tag" ]]; then
  base_tag="$(git describe --tags --abbrev=0 --match 'v[0-9]*' 2>/dev/null || true)"
fi

if [[ -z "$base_tag" ]]; then
  echo "could not find previous v* tag; pass --base TAG" >&2
  exit 1
fi

if ! git rev-parse -q --verify "refs/tags/$base_tag" >/dev/null; then
  echo "base tag does not exist: $base_tag" >&2
  exit 1
fi

if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "working tree must be clean before creating a release tag" >&2
  exit 1
fi

commit_count="$(git rev-list --count "$base_tag"..HEAD)"
if [[ "$commit_count" == "0" ]]; then
  echo "no commits found in range $base_tag..HEAD" >&2
  exit 1
fi

notes_file="$(mktemp)"
cleanup() {
  rm -f "$notes_file"
}
trap cleanup EXIT

{
  echo "Release $version"
  echo "$summary"
  echo
  echo "changes:"
  echo
  git log --reverse --pretty=format:'%s (%h)' "$base_tag"..HEAD
  echo
} > "$notes_file"

cat "$notes_file"

if [[ "$dry_run" == "1" ]]; then
  exit 0
fi

git tag -s "$version" -F "$notes_file"
git show "$version" --no-patch

if [[ "$push_tag" == "1" ]]; then
  git push origin "$version"
fi
