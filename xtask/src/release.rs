use std::env;
use std::path::PathBuf;

use crate::{Result, command_stdout, read_to_string, repo_root, run_command, write_string};

const FLATHUB_MANIFEST: &str = "packaging/flatpak/io.github.screwys.Rufin.flathub.json";

pub(crate) fn run(mut args: Vec<String>) -> Result<()> {
    if args.is_empty() {
        return Err("missing release command".into());
    }

    match args.remove(0).as_str() {
        "prepare" => prepare(args),
        "update-flathub-manifest" => update_flathub_manifest(args),
        command => Err(format!("unknown release command: {command}").into()),
    }
}

fn prepare(args: Vec<String>) -> Result<()> {
    if matches!(args.as_slice(), [arg] if arg == "-h" || arg == "--help") {
        eprintln!("Usage: cargo xtask release prepare VERSION SUMMARY");
        return Ok(());
    }

    if args.len() != 2 {
        eprintln!("Usage: cargo xtask release prepare VERSION SUMMARY");
        return Err("release prepare requires VERSION and SUMMARY".into());
    }

    let version = normalize_plain_version(&args[0])?;
    let notes = &args[1];
    if notes.is_empty() {
        return Err("release notes are required".into());
    }

    let root = repo_root()?;
    env::set_current_dir(&root)?;
    let release_date = match env::var("RELEASE_DATE") {
        Ok(date) => date,
        Err(_) => command_stdout("date", ["-u", "+%F"])?.trim().to_owned(),
    };

    replace_workspace_version(&version)?;
    run_command("cargo", ["generate-lockfile", "--offline"])?;
    update_readme_nix_refs(&version)?;
    update_metainfo_release(&version, &release_date, notes)?;
    update_issue_template_versions(&version)?;

    Ok(())
}

fn update_flathub_manifest(mut args: Vec<String>) -> Result<()> {
    let mut manifest = PathBuf::from(FLATHUB_MANIFEST);

    while !args.is_empty() {
        match args.remove(0).as_str() {
            "--manifest" => {
                if args.is_empty() {
                    return Err("--manifest requires a path".into());
                }
                manifest = PathBuf::from(args.remove(0));
            }
            "-h" | "--help" => {
                eprintln!(
                    "Usage: cargo xtask release update-flathub-manifest [--manifest PATH] TAG"
                );
                return Ok(());
            }
            "--" => break,
            arg if arg.starts_with('-') => return Err(format!("unknown option: {arg}").into()),
            arg => {
                args.insert(0, arg.to_owned());
                break;
            }
        }
    }

    if args.len() != 1 {
        return Err("release update-flathub-manifest requires TAG".into());
    }

    let tag = normalize_tag(&args[0])?;
    if !manifest.is_file() {
        return Err(format!("manifest does not exist: {}", manifest.display()).into());
    }
    ensure_tag_exists(&tag)?;

    let plain_version = tag.trim_start_matches('v');
    let commit = command_stdout("git", ["rev-list", "-n", "1", &tag])?
        .trim()
        .to_owned();
    let cargo_toml = command_stdout("git", ["show", &format!("{tag}:Cargo.toml")])?;
    let metainfo = command_stdout(
        "git",
        [
            "show",
            &format!("{tag}:data/io.github.screwys.Rufin.metainfo.xml"),
        ],
    )?;
    let cargo_version = workspace_version_from_cargo_toml(&cargo_toml)?;
    let metainfo_version = first_metainfo_release_version(&metainfo)?;

    if cargo_version != plain_version {
        return Err(format!(
            "tag {tag} has Cargo version {cargo_version}, expected {plain_version}"
        )
        .into());
    }
    if metainfo_version != plain_version {
        return Err(format!(
            "tag {tag} has MetaInfo release {metainfo_version}, expected {plain_version}"
        )
        .into());
    }

    let input = read_to_string(&manifest)?;
    let value: serde_json::Value = serde_json::from_str(&input)?;
    let modules = value
        .get("modules")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| format!("{} missing modules array", manifest.display()))?;
    let rufin = modules
        .iter()
        .find(|module| module.get("name").and_then(serde_json::Value::as_str) == Some("rufin"))
        .ok_or_else(|| format!("{} missing rufin module", manifest.display()))?;
    let sources = rufin
        .get("sources")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| format!("{} rufin module missing sources array", manifest.display()))?;
    if sources.is_empty() {
        return Err(format!("{} rufin module has no sources", manifest.display()).into());
    }

    let output = update_flathub_manifest_source_text(&input, &tag, &commit)?;
    write_string(&manifest, &output)?;
    println!("Updated {} to {} ({})", manifest.display(), tag, commit);
    Ok(())
}

fn update_flathub_manifest_source_text(input: &str, tag: &str, commit: &str) -> Result<String> {
    let name_index = input
        .find("\"name\": \"rufin\"")
        .ok_or("manifest text missing rufin module name")?;
    let sources_offset = input[name_index..]
        .find("\"sources\": [")
        .ok_or("manifest text missing rufin sources")?;
    let sources_index = name_index + sources_offset;
    let object_offset = input[sources_index..]
        .find('{')
        .ok_or("manifest text missing first rufin source object")?;
    let object_start = sources_index + object_offset;
    let object_end = find_matching_json_object(input, object_start)?;

    let line_start = input[..object_start]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    let indent = input[line_start..object_start]
        .chars()
        .take_while(|ch| ch.is_whitespace())
        .collect::<String>();
    let property_indent = format!("{indent}  ");
    let replacement = format!(
        "{{\n{property_indent}\"type\": \"git\",\n{property_indent}\"url\": \"https://github.com/screwys/Rufin.git\",\n{property_indent}\"tag\": \"{tag}\",\n{property_indent}\"commit\": \"{commit}\"\n{indent}}}"
    );

    let mut output = String::new();
    output.push_str(&input[..object_start]);
    output.push_str(&replacement);
    output.push_str(&input[object_end + 1..]);
    Ok(output)
}

fn find_matching_json_object(input: &str, object_start: usize) -> Result<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, ch) in input[object_start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth
                    .checked_sub(1)
                    .ok_or("manifest source object braces are unbalanced")?;
                if depth == 0 {
                    return Ok(object_start + offset);
                }
            }
            _ => {}
        }
    }

    Err("manifest source object is unterminated".into())
}

fn normalize_plain_version(input: &str) -> Result<String> {
    let version = input.strip_prefix('v').unwrap_or(input);
    if is_semverish(version) {
        Ok(version.to_owned())
    } else {
        Err("version must look like X.Y.Z".into())
    }
}

fn normalize_tag(input: &str) -> Result<String> {
    let tag = if input.starts_with('v') {
        input.to_owned()
    } else {
        format!("v{input}")
    };

    if is_semverish(tag.trim_start_matches('v')) {
        Ok(tag)
    } else {
        Err("tag must look like vX.Y.Z".into())
    }
}

fn is_semverish(version: &str) -> bool {
    let mut parts = version.splitn(3, '.');
    let Some(major) = parts.next() else {
        return false;
    };
    let Some(minor) = parts.next() else {
        return false;
    };
    let Some(patch_and_suffix) = parts.next() else {
        return false;
    };

    if major.is_empty()
        || minor.is_empty()
        || !major.chars().all(|ch| ch.is_ascii_digit())
        || !minor.chars().all(|ch| ch.is_ascii_digit())
    {
        return false;
    }

    let patch = patch_and_suffix
        .split(['-', '.'])
        .next()
        .unwrap_or_default();
    !patch.is_empty()
        && patch.chars().all(|ch| ch.is_ascii_digit())
        && patch_and_suffix
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '.')
}

fn ensure_tag_exists(tag: &str) -> Result<()> {
    command_stdout(
        "git",
        ["rev-parse", "-q", "--verify", &format!("refs/tags/{tag}")],
    )?;
    Ok(())
}

fn replace_workspace_version(version: &str) -> Result<()> {
    let path = PathBuf::from("Cargo.toml");
    let input = read_to_string(&path)?;
    let output = replace_workspace_version_in_toml(&input, version)?;
    write_string(&path, &output)
}

fn replace_workspace_version_in_toml(input: &str, version: &str) -> Result<String> {
    let mut output = String::new();
    let mut in_workspace_package = false;
    let mut replaced = false;

    for line in input.lines() {
        if line.starts_with('[') {
            in_workspace_package = line == "[workspace.package]";
        }

        if in_workspace_package && line.starts_with("version = ") {
            output.push_str(&format!("version = \"{version}\"\n"));
            replaced = true;
        } else {
            output.push_str(line);
            output.push('\n');
        }
    }

    if replaced {
        Ok(output)
    } else {
        Err("missing workspace package version".into())
    }
}

fn update_readme_nix_refs(version: &str) -> Result<()> {
    let path = PathBuf::from("README.md");
    let input = read_to_string(&path)?;
    let (output, count) = replace_github_version_refs(&input, version);
    if count != 2 {
        return Err(format!("expected two README Nix release refs, updated {count}").into());
    }
    write_string(&path, &output)
}

fn replace_github_version_refs(input: &str, version: &str) -> (String, usize) {
    let mut output = String::new();
    let mut count = 0;
    let needle = "github:screwys/Rufin/v";
    let bytes = input.as_bytes();
    let mut index = 0;

    while let Some(offset) = input[index..].find(needle) {
        let start = index + offset;
        output.push_str(&input[index..start + needle.len()]);
        let version_start = start + needle.len();
        let mut version_end = version_start;
        while version_end < input.len() {
            let ch = bytes[version_end] as char;
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '.' {
                version_end += 1;
            } else {
                break;
            }
        }
        output.push_str(version);
        count += 1;
        index = version_end;
    }

    output.push_str(&input[index..]);
    (output, count)
}

fn update_metainfo_release(version: &str, release_date: &str, notes: &str) -> Result<()> {
    let path = PathBuf::from("data/io.github.screwys.Rufin.metainfo.xml");
    let input = read_to_string(&path)?;
    let without_existing = remove_existing_release_entries(&input, version);
    let entry = format_metainfo_release(version, release_date, notes);
    let Some(index) = without_existing.find("  <releases>\n") else {
        return Err("missing releases section".into());
    };
    let insert_at = index + "  <releases>\n".len();
    let mut output = String::new();
    output.push_str(&without_existing[..insert_at]);
    output.push_str(&entry);
    output.push_str(&without_existing[insert_at..]);
    output = replace_raw_data_refs(&output, version);
    write_string(&path, &output)
}

fn remove_existing_release_entries(input: &str, version: &str) -> String {
    let mut output = String::new();
    let mut rest = input;
    let self_closing = format!("    <release version=\"{version}\"");

    loop {
        let Some(start) = rest.find(&self_closing) else {
            output.push_str(rest);
            break;
        };
        output.push_str(&rest[..start]);
        let candidate = &rest[start..];
        if let Some(line_end) = candidate.find('\n') {
            let line = &candidate[..line_end + 1];
            if line.trim_end().ends_with("/>") {
                rest = &candidate[line_end + 1..];
                continue;
            }
        }
        if let Some(end) = candidate.find("    </release>\n") {
            rest = &candidate[end + "    </release>\n".len()..];
        } else {
            output.push_str(candidate);
            break;
        }
    }

    output
}

fn format_metainfo_release(version: &str, release_date: &str, notes: &str) -> String {
    let mut blocks = Vec::new();
    let mut paragraph = Vec::new();
    let mut list = Vec::new();

    for line in notes.lines() {
        let line = line.trim_end();
        if let Some(item) = line.trim_start().strip_prefix("- ") {
            flush_paragraph(&mut paragraph, &mut blocks);
            let item = strip_issue_refs(item);
            if !item.is_empty() {
                list.push(item);
            }
        } else if line.trim().is_empty() {
            flush_paragraph(&mut paragraph, &mut blocks);
            flush_list(&mut list, &mut blocks);
        } else {
            flush_list(&mut list, &mut blocks);
            let item = strip_issue_refs(line);
            if !item.is_empty() {
                paragraph.push(item);
            }
        }
    }

    flush_paragraph(&mut paragraph, &mut blocks);
    flush_list(&mut list, &mut blocks);

    format!(
        "    <release version=\"{version}\" date=\"{release_date}\">\n      <description>\n{}      </description>\n    </release>\n",
        blocks.join("")
    )
}

fn flush_paragraph(paragraph: &mut Vec<String>, blocks: &mut Vec<String>) {
    if paragraph.is_empty() {
        return;
    }
    blocks.push(format!(
        "        <p>{}</p>\n",
        xml_escape(&paragraph.join("\n"))
    ));
    paragraph.clear();
}

fn flush_list(list: &mut Vec<String>, blocks: &mut Vec<String>) {
    if list.is_empty() {
        return;
    }
    blocks.push("        <ul>\n".to_owned());
    for item in list.drain(..) {
        blocks.push(format!("          <li>{}</li>\n", xml_escape(&item)));
    }
    blocks.push("        </ul>\n".to_owned());
}

fn strip_issue_refs(input: &str) -> String {
    let mut output = input.trim_end().to_owned();
    loop {
        let trimmed = output.trim_end();
        let Some((prefix, token)) = trimmed.rsplit_once(' ') else {
            break;
        };
        let token = token.trim_end_matches(['.', ',', ';', ':']);
        if is_issue_ref_token(token) {
            output = prefix.trim_end().to_owned();
        } else {
            break;
        }
    }
    output
}

fn is_issue_ref_token(token: &str) -> bool {
    let token = token
        .strip_prefix('(')
        .and_then(|value| value.strip_suffix(')'))
        .unwrap_or(token);
    let Some(hash) = token.rfind('#') else {
        return false;
    };
    let (repo, number) = token.split_at(hash);
    let number = &number[1..];
    !number.is_empty()
        && number.chars().all(|ch| ch.is_ascii_digit())
        && (repo.is_empty()
            || repo
                .split('/')
                .all(|part| !part.is_empty() && part.chars().all(is_repo_ref_char)))
}

fn is_repo_ref_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' || ch == '-'
}

fn xml_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn replace_raw_data_refs(input: &str, version: &str) -> String {
    let mut output = String::new();
    let needle = "https://raw.githubusercontent.com/screwys/Rufin/";
    let mut index = 0;

    while let Some(offset) = input[index..].find(needle) {
        let start = index + offset;
        output.push_str(&input[index..start + needle.len()]);
        let after_ref = start + needle.len();
        if let Some(data_offset) = input[after_ref..].find("/data/") {
            output.push_str(&format!("v{version}"));
            index = after_ref + data_offset;
        } else {
            output.push_str(&input[after_ref..]);
            return output;
        }
    }

    output.push_str(&input[index..]);
    output
}

fn update_issue_template_versions(version: &str) -> Result<()> {
    let path = PathBuf::from(".github/ISSUE_TEMPLATE/bug_report.yml");
    let input = read_to_string(&path)?;
    let output = update_issue_template_versions_in(&input, version)?;
    write_string(&path, &output)
}

fn update_issue_template_versions_in(input: &str, version: &str) -> Result<String> {
    let lines = input.lines().collect::<Vec<_>>();
    let Some(id_index) = lines
        .iter()
        .position(|line| line.trim() == "id: rufin-version")
    else {
        return Err("missing issue template Rufin version dropdown".into());
    };
    let Some(options_index) = lines[id_index..]
        .iter()
        .position(|line| line.trim() == "options:")
        .map(|offset| id_index + offset)
    else {
        return Err("missing issue template Rufin version options".into());
    };

    let mut end = options_index + 1;
    while end < lines.len() && lines[end].trim_start().starts_with("- ") {
        end += 1;
    }

    let mut versions = vec![version.to_owned()];
    for line in &lines[options_index + 1..end] {
        let value = line
            .trim()
            .strip_prefix("- ")
            .unwrap_or_default()
            .trim_end_matches(" (latest)");
        if value != version && is_semverish(value) && !versions.iter().any(|seen| seen == value) {
            versions.push(value.to_owned());
            if versions.len() == 6 {
                break;
            }
        }
    }

    let mut output = String::new();
    for line in &lines[..=options_index] {
        output.push_str(line);
        output.push('\n');
    }
    for entry in versions {
        output.push_str("        - ");
        output.push_str(&entry);
        if entry == version {
            output.push_str(" (latest)");
        }
        output.push('\n');
    }
    for line in &lines[end..] {
        output.push_str(line);
        output.push('\n');
    }
    Ok(output)
}

fn workspace_version_from_cargo_toml(input: &str) -> Result<String> {
    let mut in_workspace_package = false;
    for line in input.lines() {
        if line.starts_with('[') {
            in_workspace_package = line == "[workspace.package]";
        }
        if in_workspace_package && let Some(value) = quoted_value(line, "version") {
            return Ok(value);
        }
    }
    Err("missing workspace package version".into())
}

fn first_metainfo_release_version(input: &str) -> Result<String> {
    for line in input.lines() {
        let Some(start) = line.find("<release version=\"") else {
            continue;
        };
        let value_start = start + "<release version=\"".len();
        if let Some(end) = line[value_start..].find('"') {
            return Ok(line[value_start..value_start + end].to_owned());
        }
    }
    Err("missing MetaInfo release".into())
}

fn quoted_value(line: &str, key: &str) -> Option<String> {
    let prefix = format!("{key} = \"");
    line.strip_prefix(&prefix)
        .and_then(|value| value.strip_suffix('"'))
        .map(ToOwned::to_owned)
}
