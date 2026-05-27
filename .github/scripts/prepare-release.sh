#!/usr/bin/env bash
set -euo pipefail

version="${1#v}"
notes="${2:-}"

if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?$ ]]; then
  echo "version must look like X.Y.Z" >&2
  exit 1
fi

if [[ -z "$notes" ]]; then
  echo "release notes are required" >&2
  exit 1
fi

export VERSION="$version"
export RELEASE_DATE="${RELEASE_DATE:-$(date -u +%F)}"
export RELEASE_NOTES="$notes"

perl -0pi -e '
  my $version = $ENV{"VERSION"};
  die "missing workspace package version\n" unless s/(\[workspace\.package\]\nversion = )"[^"]+"/$1"$version"/m;
' Cargo.toml

perl -0pi -e '
  my $version = $ENV{"VERSION"};
  my $count = s/(^name = "rufin(?:-[^"]*)?"\nversion = )"[^"]+"/$1"$version"/mg;
  die "missing Cargo.lock Rufin package versions\n" unless $count > 0;
' Cargo.lock

perl -0pi -e '
  my $version = $ENV{"VERSION"};
  my $date = $ENV{"RELEASE_DATE"};
  my $notes = $ENV{"RELEASE_NOTES"};
  $notes =~ s/&/&amp;/g;
  $notes =~ s/</&lt;/g;
  $notes =~ s/>/&gt;/g;
  $notes =~ s/"/&quot;/g;

  s/\n    <release version="\Q$version\E"[^>]*\/>\n/\n/g;
  s/\n    <release version="\Q$version\E"[^>]*>\n.*?\n    <\/release>\n/\n/s;

  my $entry =
    qq{    <release version="$version" date="$date">\n} .
    qq{      <description>\n} .
    qq{        <p>$notes</p>\n} .
    qq{      </description>\n} .
    qq{    </release>\n};

  die "missing releases section\n" unless s/(  <releases>\n)/$1$entry/;
' data/io.github.screwys.Rufin.metainfo.xml

perl -0pi -e '
  my $version = $ENV{"VERSION"};
  s{https://raw\.githubusercontent\.com/screwys/Rufin/[^/]+/data/}{https://raw.githubusercontent.com/screwys/Rufin/v$version/data/}g;
' data/io.github.screwys.Rufin.metainfo.xml
