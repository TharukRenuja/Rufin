#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage: .github/scripts/update-aur-stable-package.sh [--check] [--skip-srcinfo] VERSION

Updates the checked-in stable AUR PKGBUILD for a release tag and refreshes
.SRCINFO when makepkg is available.
USAGE
}

check_only=0
skip_srcinfo=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --check)
      check_only=1
      shift
      ;;
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
pkgdir_rel="packaging/aur/rufin"
pkgbuild_rel="$pkgdir_rel/PKGBUILD"
srcinfo_rel="$pkgdir_rel/.SRCINFO"
pkgdir="$repo_root/$pkgdir_rel"
pkgbuild="$repo_root/$pkgbuild_rel"
srcinfo="$repo_root/$srcinfo_rel"
repo="${GITHUB_REPOSITORY:-screwys/Rufin}"
archive_url="https://github.com/${repo}/archive/refs/tags/v${version}.tar.gz"

if [[ ! -f "$pkgbuild" ]]; then
  echo "missing stable AUR PKGBUILD: $pkgbuild" >&2
  exit 1
fi

if [[ "$check_only" == "1" ]]; then
  original_pkgbuild="$(mktemp)"
  original_srcinfo="$(mktemp)"
  cp "$pkgbuild" "$original_pkgbuild"
  cp "$srcinfo" "$original_srcinfo"
  trap 'cp "$original_pkgbuild" "$pkgbuild"; cp "$original_srcinfo" "$srcinfo"; rm -f "$original_pkgbuild" "$original_srcinfo"' EXIT
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

makepkg_config_path() {
  local makepkg_path makepkg_prefix makepkg_config
  makepkg_path="$(command -v makepkg)"
  makepkg_prefix="$(dirname "$(dirname "$makepkg_path")")"
  makepkg_config="$makepkg_prefix/etc/makepkg.conf"

  if [[ -f "$makepkg_config" ]]; then
    printf '%s\n' "$makepkg_config"
  fi
}

refresh_srcinfo_with_native_makepkg() {
  if command -v makepkg >/dev/null 2>&1; then
    (
      cd "$pkgdir"
      config="$(makepkg_config_path)"
      if [[ -n "$config" ]]; then
        makepkg --config "$config" --printsrcinfo > .SRCINFO
      else
        makepkg --printsrcinfo > .SRCINFO
      fi
    )
    return 0
  fi

  return 1
}

refresh_srcinfo_with_nix_makepkg() {
  if ! command -v nix >/dev/null 2>&1; then
    return 1
  fi

  local status timeout_seconds tmp
  timeout_seconds="${RUFIN_AUR_NIX_MAKEPKG_TIMEOUT_SECONDS:-30}"
  tmp="$(mktemp)"

  if [[ ! "$timeout_seconds" =~ ^[1-9][0-9]*$ ]]; then
    echo "RUFIN_AUR_NIX_MAKEPKG_TIMEOUT_SECONDS must be a positive integer" >&2
    rm -f "$tmp"
    return 1
  fi

  set +e
  (
    cd "$repo_root"
    env -u LD_PRELOAD bash .github/scripts/retry-nix-command.sh \
      timeout "${timeout_seconds}s" env -u LD_PRELOAD nix --accept-flake-config \
        --extra-experimental-features "nix-command flakes" \
        shell nixpkgs#pacman \
        --command env -u LD_PRELOAD bash -lc '
          set -euo pipefail
          unset LD_PRELOAD
          pkgdir="$1"
          makepkg_path="$(command -v makepkg)"
          makepkg_prefix="$(dirname "$(dirname "$makepkg_path")")"
          makepkg_config="$makepkg_prefix/etc/makepkg.conf"
          if [[ ! -f "$makepkg_config" ]]; then
            echo "missing Nix pacman makepkg.conf: $makepkg_config" >&2
            exit 1
          fi
          cd "$pkgdir"
          makepkg --config "$makepkg_config" --printsrcinfo
        ' bash "$pkgdir" > "$tmp"
  )
  status="$?"
  set -e

  if [[ "$status" -eq 0 ]] &&
    [[ "$(sed -n '/[^[:space:]]/ { p; q; }' "$tmp")" == "pkgbase = rufin" ]]; then
    mv "$tmp" "$srcinfo"
    return 0
  fi

  rm -f "$tmp"
  return 1
}

refresh_srcinfo_with_makepkg() {
  refresh_srcinfo_with_native_makepkg ||
    refresh_srcinfo_with_nix_makepkg
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
    if [[ "${RUFIN_AUR_REQUIRE_MAKEPKG:-0}" == "1" ]]; then
      echo "makepkg unavailable; refusing field-only .SRCINFO update" >&2
      exit 1
    fi

    update_srcinfo_fields
    echo "makepkg unavailable; updated .SRCINFO release fields without regenerating dependencies" >&2
  fi
fi

if [[ "$check_only" == "1" ]]; then
  if cmp -s "$original_pkgbuild" "$pkgbuild" &&
    cmp -s "$original_srcinfo" "$srcinfo"; then
    exit 0
  fi

  cat >&2 <<MSG
Checked-in stable AUR metadata is not in sync with v${version}.
Run:
  .github/scripts/update-aur-stable-package.sh v${version}
MSG
  diff -u --label "$pkgbuild_rel (current)" "$original_pkgbuild" \
    --label "$pkgbuild_rel (expected)" "$pkgbuild" >&2 || true
  diff -u --label "$srcinfo_rel (current)" "$original_srcinfo" \
    --label "$srcinfo_rel (expected)" "$srcinfo" >&2 || true
  exit 1
fi
