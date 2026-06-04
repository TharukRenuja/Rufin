#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage: .github/scripts/open-flathub-pr.sh [--yes] TAG

Copies packaging/flatpak/io.github.screwys.Rufin.flathub.json into a temporary
checkout of the Flathub repository, commits it on a release branch, pushes it,
and opens or updates the matching Flathub pull request.

Environment:
  RUFIN_FLATHUB_REPO        Flathub repository slug. Default: flathub/io.github.screwys.Rufin
  RUFIN_FLATHUB_BASE        Flathub base branch. Default: master
  RUFIN_FLATHUB_PUSH_REMOTE Optional push remote URL. Default: repo clone URL
  RUFIN_FLATHUB_PR_CONFIRM  Set to 1 to skip the confirmation prompt.
USAGE
}

confirm=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --yes)
      confirm=1
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

if ! command -v gh >/dev/null 2>&1; then
  echo "gh is required to open the Flathub pull request" >&2
  exit 1
fi

if ! gh auth status >/dev/null 2>&1; then
  echo "gh must be authenticated to open the Flathub pull request" >&2
  exit 1
fi

manifest="packaging/flatpak/io.github.screwys.Rufin.flathub.json"
if [[ ! -f "$manifest" ]]; then
  echo "missing Flathub manifest: $manifest" >&2
  exit 1
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required to validate $manifest" >&2
  exit 1
fi

jq empty "$manifest"

expected_commit="$(git rev-list -n 1 "$tag")"
manifest_commit="$(
  jq -r '
    .modules[]
    | select(.name == "rufin")
    | .sources[]
    | select(type == "object")
    | select(.type == "git" and .url == "https://github.com/screwys/Rufin.git")
    | .commit // empty
  ' "$manifest"
)"

if [[ "$manifest_commit" != "$expected_commit" ]]; then
  cat >&2 <<MSG
$manifest points at $manifest_commit, expected $expected_commit for $tag.
Run .github/scripts/update-flathub-manifest.sh --manifest "$manifest" "$tag" first.
MSG
  exit 1
fi

flathub_repo="${RUFIN_FLATHUB_REPO:-flathub/io.github.screwys.Rufin}"
base_branch="${RUFIN_FLATHUB_BASE:-master}"
branch="release/$tag"
title="Update Rufin to $tag"
body=""

if ! git check-ref-format "refs/heads/$branch"; then
  echo "Flathub branch name is invalid: $branch" >&2
  exit 1
fi

cat <<MSG
Flathub pull request
Repository: $flathub_repo
Base: $base_branch
Head branch: $branch
Title: $title
Body: <empty>
MSG

if [[ "$confirm" != "1" && "${RUFIN_FLATHUB_PR_CONFIRM:-}" != "1" ]]; then
  read -r -p "Open or update this Flathub pull request? [y/N] " answer
  case "$answer" in
    y|Y|yes|YES)
      ;;
    *)
      echo "Skipped Flathub pull request."
      exit 0
      ;;
  esac
fi

tmpdir="$(mktemp -d)"
cleanup() {
  rm -rf "$tmpdir"
}
trap cleanup EXIT

clone_url="https://github.com/$flathub_repo.git"
git clone "$clone_url" "$tmpdir/flathub"
git -C "$tmpdir/flathub" switch -C "$branch" "origin/$base_branch"

cp "$manifest" "$tmpdir/flathub/io.github.screwys.Rufin.json"
cp "packaging/flatpak/cargo-sources.json" "$tmpdir/flathub/cargo-sources.json"

if [[ -f "packaging/flatpak/update-generated-sources.flathub.yml" ]]; then
  mkdir -p "$tmpdir/flathub/.github/workflows"
  cp "packaging/flatpak/update-generated-sources.flathub.yml" \
    "$tmpdir/flathub/.github/workflows/update-generated-sources.yml"
fi

if git -C "$tmpdir/flathub" diff --quiet -- \
  io.github.screwys.Rufin.json \
  cargo-sources.json \
  .github/workflows/update-generated-sources.yml; then
  echo "Flathub checkout already matches $tag."
else
  git -C "$tmpdir/flathub" add \
    io.github.screwys.Rufin.json \
    cargo-sources.json \
    .github/workflows/update-generated-sources.yml
  git -C "$tmpdir/flathub" commit -m "Update Rufin to $tag"
fi

push_remote="${RUFIN_FLATHUB_PUSH_REMOTE:-$clone_url}"
git -C "$tmpdir/flathub" remote set-url origin "$push_remote"
git -C "$tmpdir/flathub" push --force-with-lease origin "HEAD:refs/heads/$branch"

pr_number="$(
  gh pr list \
    --repo "$flathub_repo" \
    --head "$branch" \
    --state open \
    --json number \
    --jq '.[0].number // empty'
)"

if [[ -z "$pr_number" ]]; then
  gh pr create \
    --repo "$flathub_repo" \
    --base "$base_branch" \
    --head "$branch" \
    --title "$title" \
    --body "$body"
else
  gh pr edit "$pr_number" \
    --repo "$flathub_repo" \
    --title "$title" \
    --body "$body"
fi
