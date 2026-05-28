#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage: .github/scripts/update-aur-stable-package.sh [--skip-srcinfo] VERSION

Updates the checked-in stable AUR PKGBUILD for a release tag and refreshes
.SRCINFO when makepkg, or the rufin-arch Distrobox, is available.
USAGE
}

skip_srcinfo=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-srcinfo)
      skip_srcinfo=1
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
version="${version#v}"

if [[ -z "$version" ]]; then
  usage
  exit 1
fi

if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?$ ]]; then
  echo "version must look like X.Y.Z" >&2
  exit 1
fi

repo_root="$(git rev-parse --show-toplevel)"
pkgdir="$repo_root/packaging/aur/rufin"
pkgbuild="$pkgdir/PKGBUILD"
srcinfo="$pkgdir/.SRCINFO"
repo="${GITHUB_REPOSITORY:-screwys/Rufin}"
archive_url="https://github.com/${repo}/archive/refs/tags/v${version}.tar.gz"

if [[ ! -f "$pkgbuild" ]]; then
  echo "missing stable AUR PKGBUILD: $pkgbuild" >&2
  exit 1
fi

checksum=""
for attempt in 1 2 3 4 5; do
  if checksum="$(
    curl -LfsS "$archive_url" |
      sha256sum |
      awk '{print $1}'
  )"; then
    break
  fi

  if [[ "$attempt" == "5" ]]; then
    echo "failed to fetch release archive: $archive_url" >&2
    exit 1
  fi

  sleep 2
done

export VERSION="$version"
export CHECKSUM="$checksum"

perl -0pi -e '
  my $version = $ENV{"VERSION"};
  my $checksum = $ENV{"CHECKSUM"};
  my $pkgver = s/^pkgver=.*/pkgver=$version/m;
  my $sha = s/^sha256sums=.*/sha256sums=(\x27$checksum\x27)/m;
  die "missing pkgver in PKGBUILD\n" unless $pkgver == 1;
  die "missing sha256sums in PKGBUILD\n" unless $sha == 1;
' "$pkgbuild"

refresh_srcinfo_with_makepkg() {
  if command -v makepkg >/dev/null 2>&1; then
    (
      cd "$pkgdir"
      makepkg --printsrcinfo > .SRCINFO
    )
    return 0
  fi

  if command -v distrobox >/dev/null 2>&1 &&
    distrobox list 2>/dev/null | grep -Eq '(^|[[:space:]])rufin-arch([[:space:]]|$)'; then
    local pkgdir_quoted
    printf -v pkgdir_quoted '%q' "$pkgdir"
    distrobox enter --name rufin-arch -- bash -lc \
      "cd $pkgdir_quoted && makepkg --printsrcinfo > .SRCINFO"
    return 0
  fi

  return 1
}

update_srcinfo_fields() {
  if [[ ! -f "$srcinfo" ]]; then
    echo "missing stable AUR .SRCINFO: $srcinfo" >&2
    exit 1
  fi

  perl -0pi -e '
    my $version = $ENV{"VERSION"};
    my $checksum = $ENV{"CHECKSUM"};
    my $pkgver = s/^(\s*pkgver = ).*/$1$version/m;
    my $source = s{^(\s*source = rufin-)[^:]+(\.tar\.gz::https://github\.com/screwys/Rufin/archive/refs/tags/v)[^\s]+(\.tar\.gz)$}{$1$version$2$version$3}m;
    my $sha = s/^(\s*sha256sums = ).*/$1$checksum/m;
    die "missing pkgver in .SRCINFO\n" unless $pkgver == 1;
    die "missing source in .SRCINFO\n" unless $source == 1;
    die "missing sha256sums in .SRCINFO\n" unless $sha == 1;
  ' "$srcinfo"
}

if [[ "$skip_srcinfo" != "1" ]]; then
  if ! refresh_srcinfo_with_makepkg; then
    update_srcinfo_fields
    echo "makepkg unavailable; updated .SRCINFO release fields without regenerating dependencies" >&2
  fi
fi
