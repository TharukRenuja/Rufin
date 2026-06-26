#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage: .github/scripts/create-release-tag.sh [--base TAG] [--dry-run] [--replace] [--skip-flathub] VERSION SUMMARY

Updates release metadata, commits it, and creates a signed annotated tag whose
message includes commits since the previous release tag. VERSION may be vX.Y.Z
or X.Y.Z. The script also updates the checked-in Flathub manifest unless
--skip-flathub is passed.

Examples:
  .github/scripts/create-release-tag.sh --dry-run v0.2.6 "More fixes"
  .github/scripts/create-release-tag.sh v0.2.6 "More fixes"
USAGE
}

base_tag=""
dry_run=0
replace_tag=0
skip_flathub=0

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
    --replace)
      replace_tag=1
      shift
      ;;
    --skip-flathub)
      skip_flathub=1
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

release_note_extra_authors_for_pr() {
  local repo_slug="$1"
  local pr_number="$2"
  local pr_author="$3"
  local output

  if output="$(gh pr view "$pr_number" \
    --repo "$repo_slug" \
    --json commits \
    --jq '
      .commits[]
      | .authors[]
      | .login // empty
    ' 2>/dev/null)"; then
    local seen_key
    declare -A seen_key=()

    while IFS= read -r author; do
      if [[ -z "$author" ||
        "$author" == "$pr_author" ||
        -n "${seen_key[$author]+x}" ]] ||
        is_release_note_bot_author "$author"; then
        continue
      fi

      seen_key[$author]=1
      printf '%s\n' "$author"
    done <<< "$output"
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

github_author_for_commit() {
  local repo_slug="$1"
  local commit="$2"
  local output

  if output="$(gh api --method GET "repos/$repo_slug/commits/$commit" \
    --jq '.author.login // empty' \
    2>/dev/null)"; then
    printf '%s\n' "$output"
  fi
}

first_commit_for_author() {
  local repo_slug="$1"
  local author="$2"
  local output

  if output="$(gh api --method GET search/commits \
    -H 'Accept: application/vnd.github.cloak-preview+json' \
    -f q="repo:$repo_slug author:$author" \
    -f sort=author-date \
    -f order=asc \
    -f per_page=1 \
    --jq '.items[0].sha // empty' \
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
  [[ "$1" == *"[bot]" || "$1" == "weblate" ]]
}

is_release_publish_pr_title() {
  [[ "$1" == "release: publish prep for v"* ||
    "$1" == "chore(release): publish v"* ]]
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
  declare -A first_commit_by_author=()
  declare -A new_contributor_seen=()
  local new_contributors=()

  while IFS=$'\t' read -r commit subject || [[ -n "$commit" ]]; do
    case "$subject" in
      "chore(release): bump version to "*)
        continue
        ;;
      "release: publish prep for v"*)
        continue
        ;;
      "chore(flatpak): update Flathub manifest for v"*)
        continue
        ;;
      "chore(aur): update stable package for v"*)
        continue
        ;;
      "release: publish stable packages for v"*)
        continue
        ;;
      "release: sync stable package metadata for v"*)
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

        if is_release_note_bot_author "$pr_author"; then
          while IFS= read -r extra_author; do
            if [[ -n "$extra_author" ]]; then
              author_display+=", $(format_release_note_author "$extra_author")"
            fi
          done < <(release_note_extra_authors_for_pr "$repo_slug" "$pr_number" "$pr_author")
        fi

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

      if is_release_note_bot_author "$pr_author"; then
        local commit_author
        commit_author="$(github_author_for_commit "$repo_slug" "$commit")"
        if [[ -n "$commit_author" ]] && ! is_release_note_bot_author "$commit_author"; then
          if [[ -z "${first_commit_by_author[$commit_author]+x}" ]]; then
            first_commit_by_author[$commit_author]="$(first_commit_for_author "$repo_slug" "$commit_author")"
          fi

          if [[ "${first_commit_by_author[$commit_author]}" == "$commit" &&
            -z "${new_contributor_seen[$commit_author]+x}" ]]; then
            new_contributor_seen[$commit_author]=1
            new_contributors+=("$commit_author"$'\t'"$pr_number")
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
    if [[ -n "$repo_url" ]]; then
      echo
      printf '**Full Changelog:** [%s...%s](%s/compare/%s...%s)\n' \
        "$base_tag" "$version" "$repo_url" "$base_tag" "$version"
    fi
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
    env -u LD_PRELOAD bash .github/scripts/update-nix-cargo-hash.sh
    return
  fi

  cat >&2 <<'MSG'
nix is required to refresh flake.nix cargoHash during release preparation.
Install Nix before creating a release.
MSG
  exit 1
}

update_flatpak_cargo_sources() {
  local sources_script="packaging/flatpak/update-cargo-sources.sh"

  if [[ -f "$sources_script" ]]; then
    bash "$sources_script"
  fi
}

verify_nix_flake() {
  if [[ ! -f flake.nix ]]; then
    return
  fi

  env -u LD_PRELOAD bash .github/scripts/retry-nix-command.sh \
    env -u LD_PRELOAD nix --accept-flake-config \
      --extra-experimental-features "nix-command flakes" \
      flake check --no-write-lock-file --print-build-logs
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

bash .github/scripts/prepare-release.sh "$plain_version" "$summary"
update_flatpak_cargo_sources
update_nix_cargo_hash
verify_nix_flake
if ! git diff --quiet || ! git diff --cached --quiet; then
  git add Cargo.lock Cargo.toml README.md data/io.github.screwys.Rufin.metainfo.xml .github/ISSUE_TEMPLATE/bug_report.yml
  if [[ -f packaging/flatpak/cargo-sources.json ]]; then
    git add packaging/flatpak/cargo-sources.json
  fi
  if [[ -f flake.nix ]]; then
    git add flake.nix
  fi
  git commit -m "release: publish prep for $version"
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
