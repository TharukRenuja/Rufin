#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

output="${1:-locales/rufin.pot}"
existing="locales/rufin.pot"
sources="$(mktemp)"
generated="$(mktemp)"
cleanup() {
  rm -f "$sources" "$generated"
}
trap cleanup EXIT

find crates/rufin/src -name '*.rs' | sort > "$sources"

xgettext \
  --from-code=UTF-8 \
  --language=Rust \
  --package-name=Rufin \
  --msgid-bugs-address=https://github.com/screwys/Rufin/issues \
  --keyword=tr:1 \
  --keyword=text_button:2 \
  --keyword=icon_button:2 \
  --keyword=detail_action_button:2 \
  --keyword=detail_link_button:2 \
  --keyword=toggle_button:2 \
  --keyword=row_button:2 \
  --keyword=cover_hover_controls:2 \
  --keyword=relocalize_icon_button:2 \
  --keyword=dialog_button:1 \
  --keyword=labeled_control:1 \
  --keyword=labeled_row:1 \
  --keyword=smart_playlist_dialog:1 \
  --files-from="$sources" \
  --output="$generated"

python3 - "$generated" "$existing" "$output" <<'PY'
import re
import sys

generated_path, existing_path, output_path = sys.argv[1:]


def po_string(value):
    return bytes(value[1:-1], "utf-8").decode("unicode_escape")


def msgid(block):
    lines = block.splitlines()
    for index, line in enumerate(lines):
        if not line.startswith("msgid "):
            continue
        value = po_string(line[6:])
        for rest in lines[index + 1:]:
            if not rest.startswith('"'):
                break
            value += po_string(rest)
        return value
    return None


def blocks(text):
    return [block for block in re.split(r"\n{2,}", text.strip()) if block]


generated = open(generated_path, encoding="utf-8").read()
existing = open(existing_path, encoding="utf-8").read()
generated_ids = {msgid(block) for block in blocks(generated)}
fallbacks = [
    block
    for block in blocks(existing)
    if (value := msgid(block)) and value not in generated_ids
]

with open(output_path, "w", encoding="utf-8") as out:
    out.write(generated.rstrip())
    if fallbacks:
        out.write("\n\n")
        out.write("\n\n".join(fallbacks))
    out.write("\n")
PY
