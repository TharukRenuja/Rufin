#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

output="${1:-locales/rufin.pot}"
sources="$(mktemp)"
entries="$(mktemp)"
cleanup() {
  rm -f "$sources" "$entries"
}
trap cleanup EXIT

find crates/rufin/src crates/domain/src -name '*.rs' | sort > "$sources"
mkdir -p "$(dirname "$output")"

xgettext \
  --from-code=UTF-8 \
  --language=Rust \
  --no-location \
  --sort-output \
  --omit-header \
  --package-name=Rufin \
  --msgid-bugs-address=https://github.com/screwys/Rufin/issues \
  --keyword=tr:1 \
  --keyword=tr_with:1 \
  --keyword=trn:1,2 \
  --keyword=trn_with:1,2 \
  --keyword=msgid:1 \
  --keyword=text_button:2 \
  --keyword=icon_button:2 \
  --keyword=detail_action_button:2 \
  --keyword=detail_link_button:2 \
  --keyword=toggle_button:2 \
  --keyword=row_button:2 \
  --keyword=cover_hover_controls:2 \
  --keyword=relocalize_icon_button:2 \
  --keyword=context_menu_action:1 \
  --keyword=context_menu_picker_button:1 \
  --keyword=table_header_label:1 \
  --keyword=button_row:1 \
  --keyword=dialog_button:1 \
  --keyword=labeled_control:1 \
  --keyword=labeled_row:1 \
  --keyword=smart_playlist_dialog:1 \
  --files-from="$sources" \
  --output="$entries"

{
  cat <<'HEADER'
# Rufin translation template.
# Copyright (C) 2026 Rufin contributors
# This file is distributed under the same license as the Rufin package.
#
#, fuzzy
msgid ""
msgstr ""
"Project-Id-Version: Rufin\n"
"Report-Msgid-Bugs-To: https://github.com/screwys/Rufin/issues\n"
"POT-Creation-Date: YEAR-MO-DA HO:MI+ZONE\n"
"PO-Revision-Date: YEAR-MO-DA HO:MI+ZONE\n"
"Last-Translator: Rufin translators\n"
"Language-Team: Rufin translators\n"
"Language: \n"
"MIME-Version: 1.0\n"
"Content-Type: text/plain; charset=UTF-8\n"
"Content-Transfer-Encoding: 8bit\n"
HEADER
  if [ -s "$entries" ]; then
    printf '\n'
    cat "$entries"
  fi
} > "$output"
