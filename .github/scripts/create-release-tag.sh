#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage: .github/scripts/create-release-tag.sh [--base TAG] [--dry-run] [--push] [--replace] [--skip-flathub] [--skip-github-release] VERSION SUMMARY

Updates release metadata, commits it, and creates a signed annotated tag whose
message includes commits since the previous release tag. VERSION may be vX.Y.Z
or X.Y.Z. With --push, pushes main and the signed tag, then publishes the
GitHub Release from the tag using the authenticated gh user.

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

write_notes() {
  local repo_url
  repo_url="$(repo_url_from_origin)"

  {
    echo "$summary"
    echo
    echo "## Changelog"
    echo
    git log --reverse --pretty=format:'%H%x09%s' "$base_tag"..HEAD |
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

        short_commit="${commit:0:7}"
        if [[ -n "$repo_url" ]]; then
          printf -- '- %s ([%s](%s/commit/%s))\n' \
            "$subject" "$short_commit" "$repo_url" "$commit"
        else
          printf -- '- %s (%s)\n' "$subject" "$short_commit"
        fi
      done
    if [[ -n "$repo_url" ]]; then
      echo
      printf '[Full changelog](%s/compare/%s...%s)\n' "$repo_url" "$base_tag" "$version"
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
  if [[ "$skip_github_release" == "1" ]]; then
    return
  fi

  if ! command -v gh >/dev/null 2>&1; then
    cat >&2 <<'MSG'
gh is required to publish the GitHub Release after pushing the signed tag.
Install GitHub CLI, authenticate it, or pass --skip-github-release to push
the tag without publishing the release.
MSG
    exit 1
  fi

  if ! gh auth status >/dev/null 2>&1; then
    cat >&2 <<'MSG'
gh must be authenticated to publish the GitHub Release as the local user.
Run `gh auth login`, or pass --skip-github-release to push the tag without
publishing the release.
MSG
    exit 1
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
    git push origin HEAD:main
  fi
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

git tag -s "$version" -F "$notes_file"
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
  git push origin HEAD:main
  if [[ "$replace_tag" == "1" ]]; then
    git push --force origin "$version"
  else
    git push origin "$version"
  fi
  update_aur_stable_package
  publish_github_release
fi
