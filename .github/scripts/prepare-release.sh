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

cargo generate-lockfile --offline

perl -0pi -e '
  my $version = $ENV{"VERSION"};
  my $date = $ENV{"RELEASE_DATE"};
  my $notes = $ENV{"RELEASE_NOTES"};

  sub xml_escape {
    my ($text) = @_;
    $text =~ s/&/&amp;/g;
    $text =~ s/</&lt;/g;
    $text =~ s/>/&gt;/g;
    $text =~ s/"/&quot;/g;
    return $text;
  }

  sub strip_issue_refs {
    my ($text) = @_;
    my $ref = qr/(?:[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+)?#[0-9]+/;
    my $tag = qr/(?:\($ref\)|$ref)/;
    $text =~ s/(?:\s+$tag|^$tag)+(?:[.,;:])?$//;
    $text =~ s/[ \t]+$//;
    return $text;
  }

  my @description = ();
  my @paragraph = ();
  my @list = ();

  sub flush_paragraph {
    return unless @paragraph;
    push @description, "        <p>" . xml_escape(join "\n", @paragraph) . "</p>\n";
    @paragraph = ();
  }

  sub flush_list {
    return unless @list;
    push @description, "        <ul>\n";
    for my $item (@list) {
      push @description, "          <li>" . xml_escape($item) . "</li>\n";
    }
    push @description, "        </ul>\n";
    @list = ();
  }

  for my $line (split /\n/, $notes) {
    $line =~ s/[ \t]+$//;

    if ($line =~ /^\s*-\s+(.+)$/) {
      flush_paragraph();
      my $item = strip_issue_refs($1);
      push @list, $item if $item ne "";
      next;
    }

    if ($line =~ /^\s*$/) {
      flush_paragraph();
      flush_list();
      next;
    }

    flush_list();
    my $paragraph_line = strip_issue_refs($line);
    push @paragraph, $paragraph_line if $paragraph_line ne "";
  }

  flush_paragraph();
  flush_list();

  s/\n    <release version="\Q$version\E"[^>]*\/>\n/\n/g;
  s/\n    <release version="\Q$version\E"[^>]*>\n.*?\n    <\/release>\n/\n/s;

  my $entry =
    qq{    <release version="$version" date="$date">\n} .
    qq{      <description>\n} .
    join("", @description) .
    qq{      </description>\n} .
    qq{    </release>\n};

  die "missing releases section\n" unless s/(  <releases>\n)/$1$entry/;
' data/io.github.screwys.Rufin.metainfo.xml

perl -0pi -e '
  my $version = $ENV{"VERSION"};
  s{https://raw\.githubusercontent\.com/screwys/Rufin/[^/]+/data/}{https://raw.githubusercontent.com/screwys/Rufin/v$version/data/}g;
' data/io.github.screwys.Rufin.metainfo.xml

perl -0pi -e '
  my $version = $ENV{"VERSION"};
  my @existing = /^\s+- ([0-9]+\.[0-9]+\.[0-9]+(?:[-.][0-9A-Za-z.-]+)?)(?: \(latest\))?$/mg;
  my %seen = ($version => 1);
  my @versions = ($version);
  for my $existing (@existing) {
    next if $seen{$existing}++;
    push @versions, $existing;
    last if @versions == 6;
  }

  my $options = join("", map {
    "        - $_" . ($_ eq $version ? " (latest)" : "") . "\n"
  } @versions);

  my $count = s{
    (id:\ rufin-version\n\s+attributes:\n\s+label:\ Rufin\ version\n\s+options:\n)
    (?:\s+-\ .+\n)+
    (\s+default:\ 0)
  }{$1$options$2}xm;
  if ($count == 0) {
    $count = s{
      (id:\ rufin-version\n\s+attributes:\n\s+label:\ Rufin\ version\n(?:\s+(?!options:).+\n)*\s+options:\n)
      (?:\s+-\ .+\n)+
      (\s+default:\ 0)
    }{$1$options$2}xm;
  }
  die "missing issue template Rufin version dropdown\n" unless $count > 0;
' .github/ISSUE_TEMPLATE/bug_report.yml
