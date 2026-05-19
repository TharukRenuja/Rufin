#!/usr/bin/env bash
set -euo pipefail

tag_name="${1:?release tag is required}"
tag_name="${tag_name#refs/tags/}"

if [[ ! "$tag_name" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?$ ]]; then
  echo "release ref must be a vX.Y.Z tag" >&2
  exit 1
fi

if [[ "$(git rev-parse --is-shallow-repository)" == "true" ]]; then
  git fetch --force --unshallow origin
fi

git fetch --force origin \
  "refs/heads/main:refs/remotes/origin/main" \
  "refs/tags/${tag_name}:refs/tags/${tag_name}"

tag_ref="refs/tags/${tag_name}"
if [[ "$(git cat-file -t "$tag_ref")" != "tag" ]]; then
  echo "release ref must be an annotated signed tag" >&2
  exit 1
fi

if ! git cat-file tag "$tag_ref" | grep -q '^-----BEGIN PGP SIGNATURE-----$'; then
  echo "release ref must contain a PGP signature" >&2
  exit 1
fi

if [[ -f .github/release-gpg.pub ]]; then
  gnupg_home="$(mktemp -d)"
  cleanup() {
    rm -rf "$gnupg_home"
  }
  trap cleanup EXIT
  chmod 700 "$gnupg_home"
  export GNUPGHOME="$gnupg_home"
  gpg --batch --import .github/release-gpg.pub >/dev/null 2>&1
  git verify-tag "$tag_ref" >/dev/null 2>&1
fi

tag_target="$(git rev-list -n1 "$tag_ref")"
head_commit="$(git rev-parse HEAD)"
if [[ "$tag_target" != "$head_commit" ]]; then
  echo "checked-out commit does not match release tag target" >&2
  exit 1
fi

git merge-base --is-ancestor "$tag_target" origin/main
