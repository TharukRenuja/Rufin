use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

#[test]
fn i18n_template_covers_literal_ui_strings() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("app crate lives under crates/rufin-app");
    let template = parse_template(&root.join("locales/rufin.pot"));
    let sources = rust_sources(&root.join("crates/rufin-app/src"));
    let expected = expected_messages(&sources);

    let missing = expected.difference(&template).cloned().collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "locales/rufin.pot is missing translatable strings:\n{}",
        missing.join("\n")
    );
}

#[test]
fn i18n_template_has_no_duplicate_msgids() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("app crate lives under crates/rufin-app");
    let duplicates = duplicate_msgids(&root.join("locales/rufin.pot"));
    assert!(
        duplicates.is_empty(),
        "locales/rufin.pot has duplicate msgids:\n{}",
        duplicates.join("\n")
    );
}

fn expected_messages(sources: &[(PathBuf, String)]) -> BTreeSet<String> {
    let mut messages = BTreeSet::new();
    for (path, source) in sources {
        messages.extend(call_arg_strings(source, "tr", 0));

        for name in [
            "text_button",
            "icon_button",
            "detail_action_button",
            "detail_link_button",
            "toggle_button",
            "row_button",
            "cover_hover_controls",
            "relocalize_icon_button",
        ] {
            messages.extend(call_arg_strings(source, name, 1));
        }

        for name in [
            "dialog_button",
            "labeled_control",
            "labeled_row",
            "smart_playlist_dialog",
        ] {
            messages.extend(call_arg_strings(source, name, 0));
        }

        if path.ends_with("new_smart_playlist_dialog.rs") {
            messages.extend(call_arg_strings(source, "op", 1));
            messages.extend(field_strings(source, "title"));
            messages.extend(dropdown_array_strings(source));
        }
    }
    messages
}

fn rust_sources(root: &Path) -> Vec<(PathBuf, String)> {
    let mut files = Vec::new();
    collect_rust_sources(root, &mut files);
    files
        .into_iter()
        .map(|path| {
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
            (path, source)
        })
        .collect()
}

fn collect_rust_sources(dir: &Path, files: &mut Vec<PathBuf>) {
    let mut entries = fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("read {}: {error}", dir.display()))
        .flatten()
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_rust_sources(&path, files);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

fn parse_template(path: &Path) -> BTreeSet<String> {
    parse_msgids(path).into_iter().collect()
}

fn duplicate_msgids(path: &Path) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut duplicates = BTreeSet::new();
    for msgid in parse_msgids(path) {
        if !seen.insert(msgid.clone()) {
            duplicates.insert(msgid);
        }
    }
    duplicates.into_iter().collect()
}

fn parse_msgids(path: &Path) -> Vec<String> {
    let content =
        fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let lines = content.lines().collect::<Vec<_>>();
    let mut index = 0;
    let mut msgids = Vec::new();
    while index < lines.len() {
        let Some(rest) = lines[index].strip_prefix("msgid ") else {
            index += 1;
            continue;
        };
        let mut value = po_string(rest);
        index += 1;
        while index < lines.len() && lines[index].starts_with('"') {
            value.push_str(&po_string(lines[index]));
            index += 1;
        }
        if !value.is_empty() {
            msgids.push(value);
        }
    }
    msgids
}

fn call_arg_strings(source: &str, name: &str, arg_index: usize) -> Vec<String> {
    let mut strings = Vec::new();
    let mut offset = 0;
    while let Some(relative) = source[offset..].find(name) {
        let start = offset + relative;
        if !identifier_boundary(source, start, name.len()) {
            offset = start + name.len();
            continue;
        }
        let mut cursor = skip_ws(source, start + name.len());
        if source.as_bytes().get(cursor) != Some(&b'(') {
            offset = cursor;
            continue;
        }
        cursor += 1;
        if let Some(value) = nth_string_arg(source, cursor, arg_index) {
            strings.push(value);
        }
        offset = cursor;
    }
    strings
}

fn nth_string_arg(source: &str, mut cursor: usize, arg_index: usize) -> Option<String> {
    for index in 0..=arg_index {
        cursor = skip_non_string_arg(source, cursor);
        cursor = skip_ws(source, cursor);
        if index == arg_index {
            return parse_rust_string(source, cursor).map(|(value, _)| value);
        }
        let (_, next) = parse_rust_string(source, cursor)?;
        cursor = skip_ws(source, next);
        if source.as_bytes().get(cursor) != Some(&b',') {
            return None;
        }
        cursor += 1;
    }
    None
}

fn skip_non_string_arg(source: &str, mut cursor: usize) -> usize {
    let mut parens = 0;
    let mut brackets = 0;
    let mut braces = 0;
    while cursor < source.len() {
        let bytes = source.as_bytes();
        match bytes[cursor] {
            b'"' if parens == 0 && brackets == 0 && braces == 0 => return cursor,
            b'(' => parens += 1,
            b')' if parens == 0 && brackets == 0 && braces == 0 => return cursor,
            b')' => parens -= 1,
            b'[' => brackets += 1,
            b']' => brackets -= 1,
            b'{' => braces += 1,
            b'}' => braces -= 1,
            b',' if parens == 0 && brackets == 0 && braces == 0 => return cursor + 1,
            _ => {}
        }
        cursor += 1;
    }
    cursor
}

fn field_strings(source: &str, field: &str) -> Vec<String> {
    let needle = format!("{field}:");
    let mut strings = Vec::new();
    let mut offset = 0;
    while let Some(relative) = source[offset..].find(&needle) {
        let start = offset + relative + needle.len();
        let cursor = skip_ws(source, start);
        if let Some((value, _)) = parse_rust_string(source, cursor) {
            strings.push(value);
        }
        offset = start;
    }
    strings
}

fn dropdown_array_strings(source: &str) -> Vec<String> {
    let mut strings = Vec::new();
    let mut offset = 0;
    while let Some(relative) = source[offset..].find("dropdown_from_titles") {
        let start = offset + relative;
        let mut cursor = skip_ws(source, start + "dropdown_from_titles".len());
        if source.as_bytes().get(cursor) != Some(&b'(') {
            offset = cursor;
            continue;
        }
        cursor = skip_ws(source, cursor + 1);
        if source[cursor..].starts_with("&[") {
            cursor += 2;
            while let Some((value, next)) = parse_rust_string(source, skip_ws(source, cursor)) {
                strings.push(value);
                cursor = skip_ws(source, next);
                if source.as_bytes().get(cursor) == Some(&b',') {
                    cursor += 1;
                } else {
                    break;
                }
            }
        }
        offset = cursor;
    }
    strings
}

fn parse_rust_string(source: &str, start: usize) -> Option<(String, usize)> {
    if source.as_bytes().get(start) != Some(&b'"') {
        return None;
    }
    let mut value = String::new();
    let mut cursor = start + 1;
    while cursor < source.len() {
        let ch = source[cursor..].chars().next()?;
        match ch {
            '"' => return Some((value, cursor + 1)),
            '\\' => {
                cursor += ch.len_utf8();
                let escaped = source[cursor..].chars().next()?;
                match escaped {
                    'n' => value.push('\n'),
                    'r' => value.push('\r'),
                    't' => value.push('\t'),
                    '\\' => value.push('\\'),
                    '"' => value.push('"'),
                    other => value.push(other),
                }
                cursor += escaped.len_utf8();
            }
            other => {
                value.push(other);
                cursor += other.len_utf8();
            }
        }
    }
    None
}

fn po_string(value: &str) -> String {
    let value = value.trim();
    let Some(inner) = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    else {
        return String::new();
    };
    unescape_po(inner)
}

fn unescape_po(value: &str) -> String {
    let mut output = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            output.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => output.push('\n'),
            Some('r') => output.push('\r'),
            Some('t') => output.push('\t'),
            Some('"') => output.push('"'),
            Some('\\') => output.push('\\'),
            Some(other) => output.push(other),
            None => output.push('\\'),
        }
    }
    output
}

fn skip_ws(source: &str, mut cursor: usize) -> usize {
    while source
        .as_bytes()
        .get(cursor)
        .is_some_and(u8::is_ascii_whitespace)
    {
        cursor += 1;
    }
    cursor
}

fn identifier_boundary(source: &str, start: usize, len: usize) -> bool {
    let before = start
        .checked_sub(1)
        .and_then(|index| source.as_bytes().get(index))
        .is_none_or(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_');
    let after = source
        .as_bytes()
        .get(start + len)
        .is_none_or(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_');
    before && after
}
