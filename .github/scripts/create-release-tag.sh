#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage: .github/scripts/create-release-tag.sh [--base TAG] [--dry-run] [--push] [--replace] [--skip-flathub] [--skip-github-release] VERSION SUMMARY

Updates release metadata, commits it, and creates a signed annotated tag whose
message includes commits since the previous release tag. VERSION may be vX.Y.Z
or X.Y.Z. With --push, opens empty-body release PRs, waits for Checks to pass,
merges them, pushes the signed tag, gates and verifies the AUR follow-up the
same way, publishes the GitHub Release from the tag using the authenticated gh
user, then watches the release-triggered AUR, Flatpak, and Nix workflows.

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
aur_branch="$release_branch-aur"
default_branch="${RUFIN_RELEASE_DEFAULT_BRANCH:-main}"

if ! git check-ref-format "refs/heads/$release_branch"; then
  echo "release branch name is invalid: $release_branch" >&2
  exit 1
fi

if ! git check-ref-format "refs/heads/$aur_branch"; then
  echo "AUR branch name is invalid: $aur_branch" >&2
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
    --jq 'map(select(.merged_at != null)) | sort_by(.merged_at) | .[-1] | select(. != null) | [.number, .title, .user.login] | @tsv' \
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

format_release_note_author() {
  local author="$1"

  if [[ "$author" == *"[bot]" ]]; then
    local app_slug="${author%\[bot\]}"
    printf '[@%s](https://github.com/apps/%s)' "$app_slug" "$app_slug"
    return
  fi

  printf '@%s' "$author"
}

is_release_note_bot_author() {
  [[ "$1" == *"[bot]" ]]
}

is_release_publish_pr_title() {
  [[ "$1" == "chore(release): publish v"* ]]
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
      "Merge pull request #"*)
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
        if is_release_publish_pr_title "$pr_title"; then
          continue
        fi

        local author_display
        author_display="$(format_release_note_author "$pr_author")"
        printf -- '- %s by %s in #%s\n' "$pr_title" "$author_display" "$pr_number"

        if [[ -n "$pr_author" ]] && ! is_release_note_bot_author "$pr_author"; then
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
      local author_display
      author_display="$(format_release_note_author "$author")"
      printf -- '- %s made their first contribution in #%s\n' "$author_display" "$pr_number"
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
gh is required to open, watch, and merge release PRs.
Install GitHub CLI and authenticate it before using --push.
MSG
    exit 1
  fi

  if ! gh auth status >/dev/null 2>&1; then
    cat >&2 <<'MSG'
gh must be authenticated to open, watch, and merge release PRs.
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

fetch_default_branch() {
  git fetch origin "refs/heads/$default_branch:refs/remotes/origin/$default_branch"
}

check_release_push_branch() {
  local current_branch

  current_branch="$(git branch --show-current)"
  if [[ "$current_branch" != "$default_branch" ]]; then
    echo "--push releases must start from $default_branch; current branch is $current_branch" >&2
    exit 1
  fi

  fetch_default_branch
  if [[ "$(git rev-parse HEAD)" != "$(git rev-parse "origin/$default_branch")" ]]; then
    cat >&2 <<MSG
$default_branch must match origin/$default_branch before publishing a release.
Merge or discard local-only commits before using --push.
MSG
    exit 1
  fi
}

fast_forward_default_branch() {
  fetch_default_branch
  git switch "$default_branch"
  git merge --ff-only "origin/$default_branch"
}

mark_pre_pr_review() {
  local marker="${RUFIN_PR_REVIEW_GATE_MARKER:-}"

  if [[ -z "$marker" ]]; then
    local default_marker="$HOME/.codex/skills/rufin-git-workflow/scripts/rufin-pr-review-gate.py"
    if [[ -x "$default_marker" ]]; then
      marker="$default_marker"
    fi
  fi

  if [[ -z "$marker" ]]; then
    return
  fi

  printf '\nMarking reviewed release HEAD for PR creation...\n'
  "$marker" mark
}

open_release_pr() {
  local branch="$1"
  local title="$2"
  local repo_slug pr_number

  repo_slug="$(github_repo_from_origin)"
  if [[ -z "$repo_slug" ]]; then
    echo "could not determine GitHub repository from origin" >&2
    exit 1
  fi

  pr_number="$(
    gh pr list \
      --repo "$repo_slug" \
      --head "$branch" \
      --state open \
      --json number \
      --jq '.[0].number // empty'
  )"

  if [[ -z "$pr_number" ]]; then
    gh pr create \
      --repo "$repo_slug" \
      --base "$default_branch" \
      --head "$branch" \
      --title "$title" \
      --body "" >&2
    pr_number="$(
      gh pr list \
        --repo "$repo_slug" \
        --head "$branch" \
        --state open \
        --json number \
        --jq '.[0].number // empty'
    )"
  else
    gh pr edit "$pr_number" \
      --repo "$repo_slug" \
      --title "$title" \
      --body "" >&2
  fi

  if [[ -z "$pr_number" ]]; then
    echo "failed to create or find release PR for $branch" >&2
    exit 1
  fi

  printf '%s\n' "$pr_number"
}

merge_release_pr_after_checks() {
  local pr_number="$1"
  local phase="$2"
  local repo_slug

  repo_slug="$(github_repo_from_origin)"
  if [[ -z "$repo_slug" ]]; then
    echo "could not determine GitHub repository from origin" >&2
    exit 1
  fi

  printf '\nWaiting for PR #%s Checks (%s)...\n' "$pr_number" "$phase"
  gh pr checks "$pr_number" --repo "$repo_slug" --watch --fail-fast --interval 15

  printf '\nMerging PR #%s after Checks passed...\n' "$pr_number"
  gh pr merge "$pr_number" --repo "$repo_slug" --merge --delete-branch
}

push_branch_open_pr_and_merge() {
  local branch="$1"
  local title="$2"
  local phase="$3"
  local sha pr_number

  sha="$(git rev-parse HEAD)"
  printf '\nPushing %s to %s for PR Checks...\n' "$sha" "$branch"
  git push --force-with-lease origin "HEAD:refs/heads/$branch"

  mark_pre_pr_review
  pr_number="$(open_release_pr "$branch" "$title")"
  merge_release_pr_after_checks "$pr_number" "$phase"
  fast_forward_default_branch
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

release_run_id_for_workflow() {
  local workflow="$1"

  gh run list \
    --workflow "$workflow" \
    --event release \
    --branch "$version" \
    --limit 1 \
    --json databaseId \
    --jq '.[0].databaseId // empty'
}

wait_for_release_run() {
  local workflow="$1"
  local run_id=""

  for _ in 1 2 3 4 5 6 7 8 9 10 11 12 \
    13 14 15 16 17 18 19 20 21 22 23 24; do
    run_id="$(release_run_id_for_workflow "$workflow")"
    if [[ -n "$run_id" ]]; then
      printf '%s\n' "$run_id"
      return
    fi
    sleep 5
  done

  echo "could not find release workflow run for $workflow on $version" >&2
  exit 1
}

watch_release_workflows() {
  if [[ "$skip_github_release" == "1" ]]; then
    return
  fi

  local workflow run_id
  for workflow in AUR Flatpak Nix; do
    run_id="$(wait_for_release_run "$workflow")"
    printf '\nWatching %s release workflow run %s...\n' "$workflow" "$run_id"
    gh run watch "$run_id" --compact --exit-status --interval 15
  done
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

check_aur_stable_package() {
  bash .github/scripts/update-aur-stable-package.sh --check "$version"
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
  check_release_push_branch
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
  push_branch_open_pr_and_merge \
    "$release_branch" \
    "chore(release): publish $version" \
    "release metadata and Flathub manifest"
  if [[ "$replace_tag" == "1" ]]; then
    git push --force origin "$version"
  else
    git push origin "$version"
  fi
  git switch -C "$aur_branch" "origin/$default_branch"
  if update_aur_stable_package; then
    push_branch_open_pr_and_merge \
      "$aur_branch" \
      "chore(aur): publish stable package for $version" \
      "AUR stable package metadata"
  else
    fast_forward_default_branch
  fi
  check_aur_stable_package
  publish_github_release
  watch_release_workflows
fi
