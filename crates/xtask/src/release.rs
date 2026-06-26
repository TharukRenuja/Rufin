use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::Result;
use crate::generate;
use crate::process::{
    capture_command, command_stdout, find_on_path, github_repo_from_origin, read_to_string,
    repo_root, repo_url_from_origin, run_command, run_retry_without_ld_preload, temp_path,
    write_string,
};

const FLATHUB_MANIFEST: &str = "packaging/flatpak/io.github.screwys.Rufin.flathub.json";

pub(crate) fn run(mut args: Vec<String>) -> Result<()> {
    if args.is_empty() {
        return Err("missing release command".into());
    }

    match args.remove(0).as_str() {
        "prepare" => prepare(args),
        "create-tag" => create_tag(args),
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

    prepare_version(&version, notes)
}

fn prepare_version(version: &str, notes: &str) -> Result<()> {
    let root = repo_root()?;
    env::set_current_dir(&root)?;
    let release_date = match env::var("RELEASE_DATE") {
        Ok(date) => date,
        Err(_) => command_stdout("date", ["-u", "+%F"])?.trim().to_owned(),
    };

    replace_workspace_version(version)?;
    run_command("cargo", ["generate-lockfile", "--offline"])?;
    update_readme_nix_refs(version)?;
    update_metainfo_release(version, &release_date, notes)?;
    update_issue_template_versions(version)?;

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

    update_flathub_manifest_path(&manifest, &args[0])
}

fn update_flathub_manifest_path(manifest: &Path, tag: &str) -> Result<()> {
    let tag = normalize_tag(tag)?;
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

    let input = read_to_string(manifest)?;
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
    write_string(manifest, &output)?;
    println!("Updated {} to {} ({})", manifest.display(), tag, commit);
    Ok(())
}

fn create_tag(mut args: Vec<String>) -> Result<()> {
    let mut base_tag = String::new();
    let mut dry_run = false;
    let mut replace_tag = false;
    let mut skip_flathub = false;

    while !args.is_empty() {
        match args.remove(0).as_str() {
            "--base" => {
                if args.is_empty() {
                    return Err("--base requires a tag".into());
                }
                base_tag = normalize_tag(&args.remove(0))?;
            }
            "--dry-run" => dry_run = true,
            "--replace" => replace_tag = true,
            "--skip-flathub" => skip_flathub = true,
            "-h" | "--help" => {
                eprintln!(
                    "Usage: cargo xtask release create-tag [--base TAG] [--dry-run] [--replace] [--skip-flathub] VERSION SUMMARY"
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

    if args.len() != 2 {
        return Err("release create-tag requires VERSION and SUMMARY".into());
    }

    let version = normalize_tag(&args[0])?;
    let plain_version = version.trim_start_matches('v').to_owned();
    let summary = args[1].clone();
    if summary.is_empty() {
        return Err("release notes are required".into());
    }

    let root = repo_root()?;
    env::set_current_dir(&root)?;

    if !dry_run && !replace_tag && git_ref_exists(&format!("refs/tags/{version}"))? {
        return Err(format!("tag already exists: {version}").into());
    }

    if base_tag.is_empty() {
        base_tag =
            latest_release_tag()?.ok_or("could not find previous v* tag; pass --base TAG")?;
    }
    ensure_tag_exists(&base_tag)?;

    if !dry_run && !working_tree_clean()? {
        return Err("working tree must be clean before creating a release tag".into());
    }

    let commit_count =
        command_stdout("git", ["rev-list", "--count", &format!("{base_tag}..HEAD")])?;
    if commit_count.trim() == "0" {
        return Err(format!("no commits found in range {base_tag}..HEAD").into());
    }

    let mut notes = release_notes(&base_tag, &version, &summary)?;
    print_notes(&notes);
    if dry_run {
        return Ok(());
    }

    prepare_version(&plain_version, &summary)?;
    generate::flatpak_sources(false)?;
    update_nix_cargo_hash()?;
    verify_nix_flake()?;
    if !working_tree_clean()? {
        git_add_existing(&[
            "Cargo.lock",
            "Cargo.toml",
            "README.md",
            "data/io.github.screwys.Rufin.metainfo.xml",
            ".github/ISSUE_TEMPLATE/bug_report.yml",
            "packaging/flatpak/cargo-sources.json",
            "flake.nix",
        ])?;
        run_command(
            "git",
            [
                "commit",
                "-m",
                &format!("release: publish prep for {version}"),
            ],
        )?;
    }

    notes = release_notes(&base_tag, &version, &summary)?;
    print_notes(&notes);

    if replace_tag && git_ref_exists(&format!("refs/tags/{version}"))? {
        run_command("git", ["tag", "-d", &version])?;
    }

    let notes_file = temp_path("release-notes.md");
    write_string(&notes_file, &notes)?;
    run_command(
        "git",
        [
            "tag",
            "-s",
            "--cleanup=verbatim",
            &version,
            "-F",
            notes_file
                .to_str()
                .ok_or("release notes path is not valid UTF-8")?,
        ],
    )?;
    let _ = std::fs::remove_file(notes_file);
    run_command("git", ["show", &version, "--no-patch"])?;

    let flathub_manifest = PathBuf::from(FLATHUB_MANIFEST);
    if !skip_flathub && flathub_manifest.is_file() {
        update_flathub_manifest_path(&flathub_manifest, &version)?;
        if path_has_diff(&flathub_manifest)? {
            run_command("git", ["add", FLATHUB_MANIFEST])?;
            run_command(
                "git",
                [
                    "commit",
                    "-m",
                    &format!("chore(flatpak): update Flathub manifest for {version}"),
                ],
            )?;
        }
    }

    Ok(())
}

fn latest_release_tag() -> Result<Option<String>> {
    let output = capture_command(
        "git",
        ["describe", "--tags", "--abbrev=0", "--match", "v[0-9]*"],
    )?;
    if output.status.success() {
        let tag = output.stdout.trim();
        if tag.is_empty() {
            Ok(None)
        } else {
            Ok(Some(tag.to_owned()))
        }
    } else {
        Ok(None)
    }
}

fn git_ref_exists(ref_name: &str) -> Result<bool> {
    let output = capture_command("git", ["rev-parse", "-q", "--verify", ref_name])?;
    Ok(output.status.success())
}

fn working_tree_clean() -> Result<bool> {
    let diff = Command::new("git").args(["diff", "--quiet"]).status()?;
    let cached = Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .status()?;
    Ok(diff.success() && cached.success())
}

fn path_has_diff(path: &Path) -> Result<bool> {
    let status = Command::new("git")
        .args(["diff", "--quiet", "--"])
        .arg(path)
        .status()?;
    Ok(!status.success())
}

fn git_add_existing(paths: &[&str]) -> Result<()> {
    for path in paths {
        if Path::new(path).exists() {
            run_command("git", ["add", *path])?;
        }
    }
    Ok(())
}

fn update_nix_cargo_hash() -> Result<()> {
    if !Path::new("flake.nix").is_file() {
        return Ok(());
    }
    if !find_on_path("nix") {
        return Err(
            "nix is required to refresh flake.nix cargoHash during release preparation".into(),
        );
    }
    generate::nix_cargo_hash(false)
}

fn verify_nix_flake() -> Result<()> {
    if !Path::new("flake.nix").is_file() {
        return Ok(());
    }
    run_retry_without_ld_preload([
        "env",
        "-u",
        "LD_PRELOAD",
        "nix",
        "--accept-flake-config",
        "--extra-experimental-features",
        "nix-command flakes",
        "flake",
        "check",
        "--no-write-lock-file",
        "--print-build-logs",
    ])
}

fn release_notes(base_tag: &str, version: &str, summary: &str) -> Result<String> {
    let repo_url = repo_url_from_origin()?.unwrap_or_default();
    let repo_slug = github_repo_from_origin()?.unwrap_or_default();
    let use_github_metadata = github_metadata_available(&repo_slug);
    let mut changelog = ChangelogWriter::new(&repo_slug, use_github_metadata);
    let log = command_stdout(
        "git",
        [
            "log",
            "--reverse",
            "--pretty=format:%H%x09%s",
            &format!("{base_tag}..HEAD"),
        ],
    )?;

    let mut notes = String::new();
    notes.push_str(summary);
    notes.push_str("\n\n## Changelog\n\n");
    for line in log.lines() {
        let Some((commit, subject)) = line.split_once('\t') else {
            continue;
        };
        if is_release_housekeeping_subject(subject) {
            continue;
        }
        match changelog.entry_for_commit(commit, subject)? {
            ReleaseNoteEntry::Line(entry) => {
                notes.push_str(&entry);
                notes.push('\n');
            }
            ReleaseNoteEntry::Skip => {}
            ReleaseNoteEntry::Fallback if !repo_url.is_empty() => {
                notes.push_str(&format!(
                    "- {subject} ([{}]({repo_url}/commit/{commit}))\n",
                    &commit[..7]
                ));
            }
            ReleaseNoteEntry::Fallback => {
                notes.push_str(&format!("- {subject} ({})\n", &commit[..7]));
            }
        }
    }

    if !changelog.new_contributors.is_empty() {
        notes.push_str("\n## New Contributors\n\n");
        for (author, pr_number) in &changelog.new_contributors {
            notes.push_str(&format!(
                "- {} made their first contribution in #{pr_number}\n",
                format_release_note_author(author)
            ));
        }
    }

    if !repo_url.is_empty() {
        notes.push_str(&format!(
            "\n**Full Changelog:** [{base_tag}...{version}]({repo_url}/compare/{base_tag}...{version})\n"
        ));
    }
    notes.push('\n');
    Ok(notes)
}

fn print_notes(notes: &str) {
    println!("Release notes (Markdown)");
    println!();
    print!("{notes}");
}

struct ChangelogWriter<'a> {
    repo_slug: &'a str,
    use_github_metadata: bool,
    seen_prs: HashSet<String>,
    first_pr_by_author: HashMap<String, String>,
    first_commit_by_author: HashMap<String, String>,
    new_contributor_seen: HashSet<String>,
    new_contributors: Vec<(String, String)>,
}

impl<'a> ChangelogWriter<'a> {
    fn new(repo_slug: &'a str, use_github_metadata: bool) -> Self {
        Self {
            repo_slug,
            use_github_metadata,
            seen_prs: HashSet::new(),
            first_pr_by_author: HashMap::new(),
            first_commit_by_author: HashMap::new(),
            new_contributor_seen: HashSet::new(),
            new_contributors: Vec::new(),
        }
    }

    fn entry_for_commit(&mut self, commit: &str, _subject: &str) -> Result<ReleaseNoteEntry> {
        if !self.use_github_metadata {
            return Ok(ReleaseNoteEntry::Fallback);
        }

        let Some(pr) = release_note_pr_for_commit(self.repo_slug, commit)? else {
            return Ok(ReleaseNoteEntry::Fallback);
        };

        if !self.seen_prs.insert(pr.number.clone()) {
            return Ok(ReleaseNoteEntry::Skip);
        }
        if is_release_publish_pr_title(&pr.title) {
            return Ok(ReleaseNoteEntry::Skip);
        }

        let mut author_display = format_release_note_author(&pr.author);
        if is_release_note_bot_author(&pr.author) {
            for extra_author in
                release_note_extra_authors_for_pr(self.repo_slug, &pr.number, &pr.author)?
            {
                author_display.push_str(", ");
                author_display.push_str(&format_release_note_author(&extra_author));
            }
        }

        if !pr.author.is_empty() && !is_release_note_bot_author(&pr.author) {
            let first_pr = if let Some(value) = self.first_pr_by_author.get(&pr.author) {
                value.clone()
            } else {
                let value =
                    first_merged_pr_for_author(self.repo_slug, &pr.author)?.unwrap_or_default();
                self.first_pr_by_author
                    .insert(pr.author.clone(), value.clone());
                value
            };
            if first_pr == pr.number && self.new_contributor_seen.insert(pr.author.clone()) {
                self.new_contributors
                    .push((pr.author.clone(), pr.number.clone()));
            }
        }

        if is_release_note_bot_author(&pr.author)
            && let Some(commit_author) = github_author_for_commit(self.repo_slug, commit)?
            && !is_release_note_bot_author(&commit_author)
        {
            let first_commit = if let Some(value) = self.first_commit_by_author.get(&commit_author)
            {
                value.clone()
            } else {
                let value =
                    first_commit_for_author(self.repo_slug, &commit_author)?.unwrap_or_default();
                self.first_commit_by_author
                    .insert(commit_author.clone(), value.clone());
                value
            };
            if first_commit == commit && self.new_contributor_seen.insert(commit_author.clone()) {
                self.new_contributors
                    .push((commit_author, pr.number.clone()));
            }
        }

        Ok(ReleaseNoteEntry::Line(format!(
            "- {} by {} in #{}",
            pr.title, author_display, pr.number
        )))
    }
}

enum ReleaseNoteEntry {
    Line(String),
    Skip,
    Fallback,
}

struct PullRequestInfo {
    number: String,
    title: String,
    author: String,
}

fn github_metadata_available(repo_slug: &str) -> bool {
    !repo_slug.is_empty()
        && find_on_path("gh")
        && capture_command("gh", ["auth", "status"])
            .map(|output| output.status.success())
            .unwrap_or(false)
}

fn release_note_pr_for_commit(repo_slug: &str, commit: &str) -> Result<Option<PullRequestInfo>> {
    let output = capture_command(
        "gh",
        [
            "api",
            "-H",
            "Accept: application/vnd.github+json",
            &format!("repos/{repo_slug}/commits/{commit}/pulls"),
            "--jq",
            "map(select(.merged_at != null)) | sort_by(.merged_at) | .[-1] | select(. != null) | [.number, .title, .user.login] | @tsv",
        ],
    )?;
    if !output.status.success() {
        return Ok(None);
    }
    let line = output.stdout.trim();
    if line.is_empty() {
        return Ok(None);
    }
    let fields = line.splitn(3, '\t').collect::<Vec<_>>();
    if fields.len() != 3 {
        return Ok(None);
    }
    Ok(Some(PullRequestInfo {
        number: fields[0].to_owned(),
        title: fields[1].to_owned(),
        author: fields[2].to_owned(),
    }))
}

fn release_note_extra_authors_for_pr(
    repo_slug: &str,
    pr_number: &str,
    pr_author: &str,
) -> Result<Vec<String>> {
    let output = capture_command(
        "gh",
        [
            "pr",
            "view",
            pr_number,
            "--repo",
            repo_slug,
            "--json",
            "commits",
            "--jq",
            ".commits[] | .authors[] | .login // empty",
        ],
    )?;
    if !output.status.success() {
        return Ok(Vec::new());
    }
    let mut seen = HashSet::new();
    let mut authors = Vec::new();
    for author in output.stdout.lines().filter(|line| !line.is_empty()) {
        if author == pr_author || is_release_note_bot_author(author) || !seen.insert(author) {
            continue;
        }
        authors.push(author.to_owned());
    }
    Ok(authors)
}

fn first_merged_pr_for_author(repo_slug: &str, author: &str) -> Result<Option<String>> {
    github_single_value([
        "api",
        "--method",
        "GET",
        "search/issues",
        "-f",
        &format!("q=repo:{repo_slug} type:pr is:merged author:{author}"),
        "-f",
        "sort=created",
        "-f",
        "order=asc",
        "-f",
        "per_page=1",
        "--jq",
        ".items[0].number // empty",
    ])
}

fn github_author_for_commit(repo_slug: &str, commit: &str) -> Result<Option<String>> {
    github_single_value([
        "api",
        "--method",
        "GET",
        &format!("repos/{repo_slug}/commits/{commit}"),
        "--jq",
        ".author.login // empty",
    ])
}

fn first_commit_for_author(repo_slug: &str, author: &str) -> Result<Option<String>> {
    github_single_value([
        "api",
        "--method",
        "GET",
        "search/commits",
        "-H",
        "Accept: application/vnd.github.cloak-preview+json",
        "-f",
        &format!("q=repo:{repo_slug} author:{author}"),
        "-f",
        "sort=author-date",
        "-f",
        "order=asc",
        "-f",
        "per_page=1",
        "--jq",
        ".items[0].sha // empty",
    ])
}

fn github_single_value<const N: usize>(args: [&str; N]) -> Result<Option<String>> {
    let output = capture_command("gh", args)?;
    if !output.status.success() {
        return Ok(None);
    }
    let value = output.stdout.trim();
    if value.is_empty() {
        Ok(None)
    } else {
        Ok(Some(value.to_owned()))
    }
}

fn format_release_note_author(author: &str) -> String {
    if let Some(app_slug) = author.strip_suffix("[bot]") {
        format!("[@{app_slug}](https://github.com/apps/{app_slug})")
    } else {
        format!("@{author}")
    }
}

fn is_release_note_bot_author(author: &str) -> bool {
    author.ends_with("[bot]") || author == "weblate"
}

fn is_release_publish_pr_title(title: &str) -> bool {
    title.starts_with("release: publish prep for v")
        || title.starts_with("chore(release): publish v")
}

fn is_release_housekeeping_subject(subject: &str) -> bool {
    subject.starts_with("chore(release): bump version to ")
        || subject.starts_with("release: publish prep for v")
        || subject.starts_with("chore(flatpak): update Flathub manifest for v")
        || subject.starts_with("chore(aur): update stable package for v")
        || subject.starts_with("release: publish stable packages for v")
        || subject.starts_with("release: sync stable package metadata for v")
        || subject.starts_with("Merge pull request #")
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

pub(crate) fn normalize_plain_version(input: &str) -> Result<String> {
    let version = input.strip_prefix('v').unwrap_or(input);
    if is_semverish(version) {
        Ok(version.to_owned())
    } else {
        Err("version must look like X.Y.Z".into())
    }
}

pub(crate) fn normalize_tag(input: &str) -> Result<String> {
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
