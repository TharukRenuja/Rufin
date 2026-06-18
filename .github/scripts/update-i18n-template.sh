#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

output="${1:-locales/rufin.pot}"
sources="$(mktemp)"
generated="$(mktemp)"
cleanup() {
  rm -f "$sources" "$generated"
}
trap cleanup EXIT

find crates/rufin/src crates/domain/src -name '*.rs' | sort > "$sources"

xgettext \
  --from-code=UTF-8 \
  --language=Rust \
  --package-name=Rufin \
  --msgid-bugs-address=https://github.com/screwys/Rufin/issues \
  --keyword=tr:1 \
  --keyword=tr_with:1 \
  --keyword=trn:1,2 \
  --keyword=trn_with:1,2 \
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
  --output="$generated"

python3 - "$generated" "$repo_root" "$output" <<'PY'
import re
import sys
import ast
from pathlib import Path

generated_path, repo_root, output_path = sys.argv[1:]
repo_root = Path(repo_root)

HEADER = '''# Rufin translation template.
# Copyright (C) 2026 Rufin contributors
# This file is distributed under the same license as the Rufin package.
#
#, fuzzy
msgid ""
msgstr ""
"Project-Id-Version: Rufin\\n"
"Report-Msgid-Bugs-To: https://github.com/screwys/Rufin/issues\\n"
"POT-Creation-Date: YEAR-MO-DA HO:MI+ZONE\\n"
"PO-Revision-Date: YEAR-MO-DA HO:MI+ZONE\\n"
"Last-Translator: Rufin translators\\n"
"Language-Team: Rufin translators\\n"
"Language: \\n"
"MIME-Version: 1.0\\n"
"Content-Type: text/plain; charset=UTF-8\\n"
"Content-Transfer-Encoding: 8bit\\n"
'''


def po_string(value):
    return ast.literal_eval(value)


def po_quote(value):
    value = (
        value.replace("\\", "\\\\")
        .replace("\n", "\\n")
        .replace("\r", "\\r")
        .replace("\t", "\\t")
        .replace('"', '\\"')
    )
    return f'"{value}"'


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


def msgid_plural(block):
    lines = block.splitlines()
    for index, line in enumerate(lines):
        if not line.startswith("msgid_plural "):
            continue
        value = po_string(line[13:])
        for rest in lines[index + 1:]:
            if not rest.startswith('"'):
                break
            value += po_string(rest)
        return value
    return None


def blocks(text):
    return [block for block in re.split(r"\n{2,}", text.strip()) if block]


def line_number(source, index):
    return source.count("\n", 0, index) + 1


def skip_ws(source, cursor):
    while cursor < len(source) and source[cursor].isspace():
        cursor += 1
    return cursor


def identifier_boundary(source, start, length):
    before = start == 0 or not (source[start - 1].isalnum() or source[start - 1] == "_")
    after_index = start + length
    after = after_index >= len(source) or not (
        source[after_index].isalnum() or source[after_index] == "_"
    )
    return before and after


def parse_rust_string(source, start):
    if start >= len(source) or source[start] != '"':
        return None
    value = []
    cursor = start + 1
    while cursor < len(source):
        ch = source[cursor]
        if ch == '"':
            return ("".join(value), cursor + 1)
        if ch == "\\":
            cursor += 1
            if cursor >= len(source):
                value.append("\\")
                break
            escaped = source[cursor]
            value.append({"n": "\n", "r": "\r", "t": "\t"}.get(escaped, escaped))
        else:
            value.append(ch)
        cursor += 1
    return None


def skip_non_string_arg(source, cursor):
    parens = brackets = braces = 0
    while cursor < len(source):
        ch = source[cursor]
        if ch == '"' and parens == brackets == braces == 0:
            return cursor
        if ch == "(":
            parens += 1
        elif ch == ")" and parens == brackets == braces == 0:
            return cursor
        elif ch == ")":
            parens -= 1
        elif ch == "[":
            brackets += 1
        elif ch == "]":
            brackets -= 1
        elif ch == "{":
            braces += 1
        elif ch == "}":
            braces -= 1
        elif ch == "," and parens == brackets == braces == 0:
            return cursor + 1
        cursor += 1
    return cursor


def nth_string_arg(source, cursor, arg_index):
    for _ in range(arg_index):
        cursor = skip_arg(source, cursor)
        if cursor is None:
            return None
    cursor = skip_non_string_arg(source, cursor)
    cursor = skip_ws(source, cursor)
    parsed = parse_rust_string(source, cursor)
    return parsed[0] if parsed else None


def skip_arg(source, cursor):
    parens = brackets = braces = 0
    while cursor < len(source):
        ch = source[cursor]
        if ch == '"':
            parsed = parse_rust_string(source, cursor)
            cursor = parsed[1] if parsed else cursor + 1
            continue
        if ch == "(":
            parens += 1
        elif ch == ")" and parens == brackets == braces == 0:
            return None
        elif ch == ")":
            parens -= 1
        elif ch == "[":
            brackets += 1
        elif ch == "]":
            brackets -= 1
        elif ch == "{":
            braces += 1
        elif ch == "}":
            braces -= 1
        elif ch == "," and parens == brackets == braces == 0:
            return cursor + 1
        cursor += 1
    return None


def call_arg_strings(source, name, arg_index):
    strings = []
    offset = 0
    while True:
        start = source.find(name, offset)
        if start < 0:
            return strings
        if not identifier_boundary(source, start, len(name)):
            offset = start + len(name)
            continue
        cursor = skip_ws(source, start + len(name))
        if cursor >= len(source) or source[cursor] != "(":
            offset = cursor
            continue
        value = nth_string_arg(source, cursor + 1, arg_index)
        if value is not None:
            strings.append((value, start))
        offset = cursor + 1


def field_strings(source, field):
    strings = []
    needle = f"{field}:"
    offset = 0
    while True:
        start = source.find(needle, offset)
        if start < 0:
            return strings
        cursor = skip_ws(source, start + len(needle))
        parsed = parse_rust_string(source, cursor)
        if parsed:
            strings.append((parsed[0], cursor))
        offset = start + len(needle)


def dropdown_array_strings(source):
    strings = []
    offset = 0
    while True:
        start = source.find("dropdown_from_titles", offset)
        if start < 0:
            return strings
        cursor = skip_ws(source, start + len("dropdown_from_titles"))
        if cursor >= len(source) or source[cursor] != "(":
            offset = cursor
            continue
        cursor = skip_ws(source, cursor + 1)
        if source.startswith("&[", cursor):
            cursor += 2
            while True:
                parsed = parse_rust_string(source, skip_ws(source, cursor))
                if not parsed:
                    break
                value, next_cursor = parsed
                strings.append((value, cursor))
                cursor = skip_ws(source, next_cursor)
                if cursor < len(source) and source[cursor] == ",":
                    cursor += 1
                else:
                    break
        offset = cursor


def matching_brace(source, open_index):
    depth = 0
    for cursor in range(open_index, len(source)):
        ch = source[cursor]
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                return cursor
    return None


def arrow_strings(source, start):
    strings = []
    offset = 0
    while True:
        arrow = source.find("=>", offset)
        if arrow < 0:
            return strings
        cursor = skip_ws(source, arrow + 2)
        parsed = parse_rust_string(source, cursor)
        if parsed:
            strings.append((parsed[0], start + cursor))
        offset = cursor + 1


def function_return_strings(source, name):
    strings = []
    needle = f"fn {name}"
    offset = 0
    while True:
        start = source.find(needle, offset)
        if start < 0:
            return strings
        if not identifier_boundary(source, start + 3, len(name)):
            offset = start + len(needle)
            continue
        open_index = source.find("{", start)
        if open_index < 0:
            return strings
        close_index = matching_brace(source, open_index)
        if close_index is None:
            return strings
        strings.extend(arrow_strings(source[open_index : close_index + 1], open_index))
        offset = close_index + 1


def source_messages():
    messages = {}
    roots = [repo_root / "crates/rufin/src", repo_root / "crates/domain/src"]
    paths = sorted(path for root in roots for path in root.rglob("*.rs"))
    for path in paths:
        rel = path.relative_to(repo_root).as_posix()
        source = path.read_text(encoding="utf-8")
        items = []
        items.extend(call_arg_strings(source, "tr", 0))
        items.extend(call_arg_strings(source, "tr_with", 0))
        for name in ["trn", "trn_with"]:
            items.extend(call_arg_strings(source, name, 0))
            items.extend(call_arg_strings(source, name, 1))
        for name in [
            "text_button",
            "icon_button",
            "detail_action_button",
            "detail_link_button",
            "toggle_button",
            "row_button",
            "cover_hover_controls",
            "relocalize_icon_button",
        ]:
            items.extend(call_arg_strings(source, name, 1))
        for name in [
            "button_row",
            "context_menu_action",
            "context_menu_picker_button",
            "dialog_button",
            "labeled_control",
            "labeled_row",
            "smart_playlist_dialog",
            "table_header_label",
        ]:
            items.extend(call_arg_strings(source, name, 0))
        items.extend(field_strings(source, "empty_body"))
        if rel.endswith("new_smart_playlist_dialog.rs"):
            items.extend(call_arg_strings(source, "op", 1))
            items.extend(field_strings(source, "title"))
            items.extend(dropdown_array_strings(source))
        if (
            rel.endswith("domain.rs")
            or rel.endswith("route.rs")
            or rel.endswith("settings/layout.rs")
            or rel.endswith("settings/sidebar.rs")
            or rel.endswith("root/layout_rendering.rs")
        ):
            items.extend(function_return_strings(source, "title"))
        if rel.endswith("cards.rs"):
            items.extend(function_return_strings(source, "field_group_title"))
            items.extend(function_return_strings(source, "layout_title"))
        for value, index in items:
            if value == "#":
                continue
            messages.setdefault(value, set()).add(f"{rel}:{line_number(source, index)}")
    return messages


def generated_blocks(text):
    parsed = []
    generated_ids = set()
    for block in blocks(text):
        value = msgid(block)
        if not value:
            continue
        parsed.append(block)
        generated_ids.add(value)
        plural = msgid_plural(block)
        if plural:
            generated_ids.add(plural)
    return parsed, generated_ids


def normalize_block_refs(block, locations):
    if not locations:
        return block
    lines = [line for line in block.splitlines() if not line.startswith("#: ")]
    return "\n".join(["#: " + " ".join(sorted(locations))] + lines)


def source_block(value, locations):
    lines = []
    if locations:
        lines.append("#: " + " ".join(sorted(locations)))
    lines.append(f"msgid {po_quote(value)}")
    lines.append('msgstr ""')
    return "\n".join(lines)


generated = open(generated_path, encoding="utf-8").read()
generated_entries, generated_ids = generated_blocks(generated)
messages = source_messages()
generated_entries = [
    normalize_block_refs(block, messages.get(msgid(block)))
    for block in generated_entries
]
extra_entries = [
    source_block(value, locations)
    for value, locations in sorted(messages.items())
    if value not in generated_ids
]

with open(output_path, "w", encoding="utf-8") as out:
    out.write(HEADER.rstrip())
    for block in sorted(generated_entries + extra_entries, key=msgid):
        out.write("\n\n")
        out.write(block.rstrip())
    out.write("\n")
PY
