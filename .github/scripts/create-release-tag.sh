#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage: .github/scripts/create-release-tag.sh [--base TAG] [--dry-run] [--push] [--replace] [--skip-flathub] [--skip-github-release] VERSION SUMMARY

Updates release metadata, commits it, and creates a signed annotated tag whose
message includes commits since the previous release tag. VERSION may be vX.Y.Z
or X.Y.Z. With --push, pushes release changes to a temporary branch, waits for
Checks to pass, fast-forwards main, pushes the signed tag, gates the AUR follow-up
the same way, then publishes the GitHub Release from the tag using the
authenticated gh user.

Examples:
  .github/scripts/create-release-tag.sh --dry-run v0.2.6 "More fixes"
  .github/scripts/create-release-tag.sh --push v0.2.6 "More fixes"
USAGE
}

base_tag=""
dry_run=0
push_tag=0
replace_tag=0
skip_flathub=0
skip_github_release=0

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
    --replace)
      replace_tag=1
      shift
      ;;
    --skip-flathub)
      skip_flathub=1
      shift
      ;;
    --skip-github-release)
      skip_github_release=1
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
plain_version="${version#v}"
release_branch="release/$version"
checks_workflow="checks.yml"
checks_timeout_seconds=7200

if ! git check-ref-format "refs/heads/$release_branch"; then
  echo "release branch name is invalid: $release_branch" >&2
  exit 1
fi

if [[ "$dry_run" != "1" && "$replace_tag" != "1" ]] &&
  git rev-parse -q --verify "refs/tags/$version" >/dev/null; then
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

if [[ "$dry_run" != "1" ]] && { ! git diff --quiet || ! git diff --cached --quiet; }; then
  echo "working tree must be clean before creating a release tag" >&2
  exit 1
fi

notes_file="$(mktemp)"
cleanup() {
  rm -f "$notes_file"
}
trap cleanup EXIT

repo_url_from_origin() {
  local origin
  origin="$(git config --get remote.origin.url || true)"
  case "$origin" in
    git@github.com:*)
      origin="https://github.com/${origin#git@github.com:}"
      ;;
    ssh://git@github.com/*)
      origin="https://github.com/${origin#ssh://git@github.com/}"
      ;;
  esac
  origin="${origin%.git}"

  if [[ "$origin" == https://github.com/*/* ]]; then
    printf '%s\n' "$origin"
  fi
}

github_repo_from_origin() {
  local repo_url
  repo_url="$(repo_url_from_origin)"

  if [[ "$repo_url" == https://github.com/*/* ]]; then
    printf '%s\n' "${repo_url#https://github.com/}"
  fi
}

github_metadata_available() {
  [[ -n "${1:-}" ]] &&
    command -v gh >/dev/null 2>&1 &&
    gh auth status >/dev/null 2>&1
}

release_note_pr_for_commit() {
  local repo_slug="$1"
  local commit="$2"
  local output

  if output="$(gh api -H 'Accept: application/vnd.github+json' \
    "repos/$repo_slug/commits/$commit/pulls" \
    --jq 'if length == 0 then empty else ((map(select(.merged_at != null)) | sort_by(.merged_at) | .[-1]) // .[0]) | [.number, .title, .user.login] | @tsv end' \
    2>/dev/null)"; then
    printf '%s\n' "$output"
  fi
}

first_merged_pr_for_author() {
  local repo_slug="$1"
  local author="$2"
  local output

  if output="$(gh api --method GET search/issues \
    -f q="repo:$repo_slug type:pr is:merged author:$author" \
    -f sort=created \
    -f order=asc \
    -f per_page=1 \
    --jq '.items[0].number // empty' \
    2>/dev/null)"; then
    printf '%s\n' "$output"
  fi
}

write_changelog() {
  local repo_url="$1"
  local repo_slug="$2"
  local use_github_metadata=0

  if github_metadata_available "$repo_slug"; then
    use_github_metadata=1
  fi

  declare -A seen_prs=()
  declare -A first_pr_by_author=()
  declare -A new_contributor_seen=()
  local new_contributors=()

  while IFS=$'\t' read -r commit subject || [[ -n "$commit" ]]; do
    case "$subject" in
      "chore(release): bump version to "*)
        continue
        ;;
      "chore(flatpak): update Flathub manifest for v"*)
        continue
        ;;
      "chore(aur): update stable package for v"*)
        continue
        ;;
    esac

    local pr_info=""
    if [[ "$use_github_metadata" == "1" ]]; then
      pr_info="$(release_note_pr_for_commit "$repo_slug" "$commit")"
    fi

    if [[ -n "$pr_info" ]]; then
      local pr_number pr_title pr_author
      IFS=$'\t' read -r pr_number pr_title pr_author <<< "$pr_info"

      if [[ -z "${seen_prs[$pr_number]+x}" ]]; then
        seen_prs[$pr_number]=1
        printf -- '- %s by @%s in #%s\n' "$pr_title" "$pr_author" "$pr_number"

        if [[ -n "$pr_author" ]]; then
          if [[ -z "${first_pr_by_author[$pr_author]+x}" ]]; then
            first_pr_by_author[$pr_author]="$(first_merged_pr_for_author "$repo_slug" "$pr_author")"
          fi

          if [[ "${first_pr_by_author[$pr_author]}" == "$pr_number" &&
            -z "${new_contributor_seen[$pr_author]+x}" ]]; then
            new_contributor_seen[$pr_author]=1
            new_contributors+=("$pr_author"$'\t'"$pr_number")
          fi
        fi
      fi
      continue
    fi

    short_commit="${commit:0:7}"
    if [[ -n "$repo_url" ]]; then
      printf -- '- %s ([%s](%s/commit/%s))\n' \
        "$subject" "$short_commit" "$repo_url" "$commit"
    else
      printf -- '- %s (%s)\n' "$subject" "$short_commit"
    fi
  done < <(git log --reverse --pretty=format:'%H%x09%s' "$base_tag"..HEAD)

  if [[ "${#new_contributors[@]}" -gt 0 ]]; then
    echo
    echo "## New Contributors"
    echo
    local contributor
    for contributor in "${new_contributors[@]}"; do
      local author pr_number
      IFS=$'\t' read -r author pr_number <<< "$contributor"
      printf -- '- @%s made their first contribution in #%s\n' "$author" "$pr_number"
    done
  fi
}

write_notes() {
  local repo_url repo_slug
  repo_url="$(repo_url_from_origin)"
  repo_slug="$(github_repo_from_origin)"

  {
    echo "$summary"
    echo
    echo "## Changelog"
    echo
    write_changelog "$repo_url" "$repo_slug"
    echo
  } > "$notes_file"
}

print_notes() {
  echo "Release notes (Markdown)"
  echo
  cat "$notes_file"
}

update_nix_cargo_hash() {
  if [[ ! -f flake.nix ]]; then
    return
  fi

  if command -v nix >/dev/null 2>&1; then
    bash .github/scripts/update-nix-cargo-hash.sh
    return
  fi

  if command -v distrobox >/dev/null 2>&1 &&
    distrobox list 2>/dev/null | grep -Eq '(^|[[:space:]])rufin-arch([[:space:]]|$)'; then
    local root_quoted
    printf -v root_quoted '%q' "$(pwd)"
    distrobox enter --name rufin-arch -- bash -lc \
      "cd $root_quoted && bash .github/scripts/update-nix-cargo-hash.sh"
    return
  fi

  cat >&2 <<'MSG'
nix is required to refresh flake.nix cargoHash during release preparation.
Install Nix, or make sure the rufin-arch Distrobox is available with Nix
installed, before creating a release.
MSG
  exit 1
}

check_github_release_prereqs() {
  if ! command -v gh >/dev/null 2>&1; then
    cat >&2 <<'MSG'
gh is required to wait for release Checks before pushing main.
Install GitHub CLI and authenticate it before using --push.
MSG
    exit 1
  fi

  if ! gh auth status >/dev/null 2>&1; then
    cat >&2 <<'MSG'
gh must be authenticated to wait for release Checks before pushing main.
Run `gh auth login` before using --push.
MSG
    exit 1
  fi

  if [[ "$skip_github_release" == "1" ]]; then
    return
  fi

  if gh release view "$version" >/dev/null 2>&1; then
    cat >&2 <<MSG
GitHub Release already exists for $version.
Delete or edit the existing release, or pass --skip-github-release to skip
publishing from this script.
MSG
    exit 1
  fi
}

wait_for_release_checks() {
  local sha="$1"
  local phase="$2"
  local repo_slug run_id run_url deadline

  repo_slug="$(github_repo_from_origin)"
  if [[ -z "$repo_slug" ]]; then
    echo "could not determine GitHub repository from origin" >&2
    exit 1
  fi

  deadline=$((SECONDS + checks_timeout_seconds))
  printf '\nWaiting for Checks on %s (%s) via %s...\n' "$sha" "$phase" "$release_branch"

  while true; do
    run_id="$(
      gh run list \
        --repo "$repo_slug" \
        --workflow "$checks_workflow" \
        --branch "$release_branch" \
        --commit "$sha" \
        --event push \
        --limit 1 \
        --json databaseId \
        --jq '.[0].databaseId // empty'
    )"

    if [[ -n "$run_id" ]]; then
      run_url="$(
        gh run view "$run_id" \
          --repo "$repo_slug" \
          --json url \
          --jq '.url'
      )"
      printf 'Watching %s\n' "$run_url"
      gh run watch "$run_id" --repo "$repo_slug" --interval 15 --exit-status
      return
    fi

    if (( SECONDS >= deadline )); then
      echo "timed out waiting for Checks to start for $sha on $release_branch" >&2
      exit 1
    fi

    sleep 10
  done
}

push_release_branch_and_wait() {
  local phase="$1"
  local sha

  sha="$(git rev-parse HEAD)"
  printf '\nPushing %s to %s for CI preflight...\n' "$sha" "$release_branch"
  git push --force-with-lease origin "HEAD:refs/heads/$release_branch"
  wait_for_release_checks "$sha" "$phase"
}

push_main_after_checks() {
  local phase="$1"

  push_release_branch_and_wait "$phase"
  printf '\nFast-forwarding main to %s after Checks passed...\n' "$(git rev-parse HEAD)"
  git push origin HEAD:main
}

delete_release_branch() {
  printf '\nDeleting temporary release branch %s...\n' "$release_branch"
  git push origin ":refs/heads/$release_branch" >/dev/null 2>&1 || true
}

publish_github_release() {
  if [[ "$skip_github_release" == "1" ]]; then
    printf '\nPushed signed tag %s. Skipped GitHub Release publication.\n' "$version"
    return
  fi

  gh release create "$version" \
    --title "$version" \
    --notes-from-tag \
    --verify-tag
  printf '\nPublished GitHub Release %s with the authenticated gh user.\n' "$version"
}

update_aur_stable_package() {
  bash .github/scripts/update-aur-stable-package.sh "$version"
  if ! git diff --quiet -- packaging/aur/rufin/PKGBUILD packaging/aur/rufin/.SRCINFO; then
    git add packaging/aur/rufin/PKGBUILD packaging/aur/rufin/.SRCINFO
    git commit -m "chore(aur): update stable package for $version"
    return 0
  fi

  return 1
}

commit_count="$(git rev-list --count "$base_tag"..HEAD)"
if [[ "$commit_count" == "0" ]]; then
  echo "no commits found in range $base_tag..HEAD" >&2
  exit 1
fi

write_notes

print_notes

if [[ "$dry_run" == "1" ]]; then
  exit 0
fi

if [[ "$push_tag" == "1" ]]; then
  check_github_release_prereqs
fi

bash .github/scripts/prepare-release.sh "$plain_version" "$summary"
update_nix_cargo_hash
if ! git diff --quiet || ! git diff --cached --quiet; then
  git add Cargo.lock Cargo.toml data/io.github.screwys.Rufin.metainfo.xml
  if [[ -f flake.nix ]]; then
    git add flake.nix
  fi
  git commit -m "chore(release): bump version to $plain_version"
fi

write_notes
print_notes

if [[ "$replace_tag" == "1" ]] && git rev-parse -q --verify "refs/tags/$version" >/dev/null; then
  git tag -d "$version"
fi

# Keep Markdown headings such as "## Changelog"; git tag's default cleanup
# treats lines starting with # as comments.
git tag -s --cleanup=verbatim "$version" -F "$notes_file"
git show "$version" --no-patch

flathub_manifest="packaging/flatpak/io.github.screwys.Rufin.flathub.json"
if [[ "$skip_flathub" != "1" && -f "$flathub_manifest" ]]; then
  bash .github/scripts/update-flathub-manifest.sh --manifest "$flathub_manifest" "$version"
  if ! git diff --quiet -- "$flathub_manifest"; then
    git add "$flathub_manifest"
    git commit -m "chore(flatpak): update Flathub manifest for $version"
  fi
fi

if [[ "$push_tag" == "1" ]]; then
  push_main_after_checks "release metadata and Flathub manifest"
  if [[ "$replace_tag" == "1" ]]; then
    git push --force origin "$version"
  else
    git push origin "$version"
  fi
  if update_aur_stable_package; then
    push_main_after_checks "AUR stable package metadata"
  fi
  publish_github_release
  delete_release_branch
fi
