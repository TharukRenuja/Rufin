use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::process::{
    collect_files_relative, command_stdout, ensure_command, path_to_slash, read_to_string,
    repo_root, run_command, temp_path,
};
use crate::release::normalize_tag;
use crate::{Result, ensure_no_args, print_help_if_requested};

pub(crate) fn run(mut args: Vec<String>) -> Result<()> {
    if args.is_empty() {
        return Err("missing verify command".into());
    }

    match args.remove(0).as_str() {
        "icons" => {
            if print_help_if_requested(&args, "Usage: cargo run --locked -p xtask -- verify icons")?
            {
                return Ok(());
            }
            ensure_no_args(&args)?;
            icons()
        }
        "package-layout" => package_layout(args),
        "release-tag" => release_tag(args),
        command => Err(format!("unknown verify command: {command}").into()),
    }
}

pub(crate) fn icons() -> Result<()> {
    let root = repo_root()?;
    let icon_root = root.join("data/icons/hicolor");
    let manifests = [
        root.join("packaging/flatpak/io.github.screwys.Rufin.json"),
        root.join("packaging/flatpak/io.github.screwys.Rufin.flathub.json"),
    ];

    let mut icon_paths = Vec::new();
    collect_files_relative(&icon_root, &icon_root, &mut icon_paths)?;
    icon_paths.sort();
    if icon_paths.is_empty() {
        return Err(format!("no icons found in {}", icon_root.display()).into());
    }

    let mut errors = Vec::new();
    for manifest in manifests {
        if !manifest.is_file() {
            errors.push(format!("missing Flatpak manifest: {}", manifest.display()));
            continue;
        }

        let build_commands = flatpak_rufin_build_commands(&manifest)?;
        for icon_path in &icon_paths {
            let assertion = format!(
                "test -f /app/share/icons/hicolor/{}",
                path_to_slash(icon_path)
            );
            if !build_commands.contains(&assertion) {
                errors.push(format!(
                    "{} missing icon assertion: {}",
                    manifest.display(),
                    assertion
                ));
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        for error in errors {
            eprintln!("{error}");
        }
        Err("Flatpak icon assertions are incomplete".into())
    }
}

fn flatpak_rufin_build_commands(manifest: &Path) -> Result<HashSet<String>> {
    let value: serde_json::Value = serde_json::from_str(&read_to_string(manifest)?)?;
    let modules = value
        .get("modules")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| format!("{} missing modules array", manifest.display()))?;

    let rufin = modules
        .iter()
        .find(|module| module.get("name").and_then(serde_json::Value::as_str) == Some("rufin"))
        .ok_or_else(|| format!("{} missing rufin module", manifest.display()))?;

    let commands = rufin
        .get("build-commands")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| format!("{} missing rufin build-commands", manifest.display()))?;

    Ok(commands
        .iter()
        .filter_map(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
        .collect())
}

fn package_layout(args: Vec<String>) -> Result<()> {
    if matches!(args.as_slice(), [arg] if arg == "-h" || arg == "--help") {
        eprintln!("Usage: cargo run --locked -p xtask -- verify package-layout ROOT [PREFIX]");
        return Ok(());
    }
    if args.is_empty() || args.len() > 2 {
        return Err("verify package-layout requires ROOT and optional PREFIX".into());
    }

    let root = trim_root(&args[0]);
    let prefix = trim_prefix(args.get(1).map(String::as_str).unwrap_or("/usr"));

    if prefix.is_empty() {
        let unix_bin = package_path(&root, &prefix, "bin/rufin");
        let windows_bin = package_path(&root, &prefix, "bin/rufin.exe");
        if !unix_bin.is_file() && !windows_bin.is_file() {
            return Err(format!("missing executable under {}", args[0]).into());
        }
    } else {
        require_file(&package_path(&root, &prefix, "bin/rufin"))?;
    }

    require_file(&package_path(
        &root,
        &prefix,
        "share/applications/io.github.screwys.Rufin.desktop",
    ))?;
    require_file(&package_path(
        &root,
        &prefix,
        "share/metainfo/io.github.screwys.Rufin.metainfo.xml",
    ))?;
    require_file(&package_path(
        &root,
        &prefix,
        "share/rufin/japanese-readings.dic",
    ))?;
    require_file(&package_path(
        &root,
        &prefix,
        "share/licenses/rufin/japanese-readings.LICENSE",
    ))?;

    let repo = workspace_source_root()?;
    let icon_root = repo.join("data/icons/hicolor");
    let mut icon_paths = Vec::new();
    collect_files_relative(&icon_root, &icon_root, &mut icon_paths)?;
    icon_paths.sort();
    for icon in icon_paths {
        require_file(&package_path(
            &root,
            &prefix,
            &format!("share/icons/hicolor/{}", path_to_slash(&icon)),
        ))?;
    }

    for po_file in po_files(&repo.join("crates/localization/locales"))? {
        let lang = po_file
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(|| format!("invalid locale filename: {}", po_file.display()))?;
        require_file(&package_path(
            &root,
            &prefix,
            &format!("share/locale/{lang}/LC_MESSAGES/rufin.mo"),
        ))?;
    }

    Ok(())
}

fn trim_root(root: &str) -> String {
    let root = root.trim_end_matches('/').to_owned();
    if root == "/" { String::new() } else { root }
}

fn trim_prefix(prefix: &str) -> String {
    let prefix = format!("/{}", prefix.trim_start_matches('/'))
        .trim_end_matches('/')
        .to_owned();
    if prefix == "/" { String::new() } else { prefix }
}

fn package_path(root: &str, prefix: &str, path: &str) -> PathBuf {
    PathBuf::from(format!("{root}{prefix}/{path}"))
}

fn require_file(path: &Path) -> Result<()> {
    if path.is_file() {
        Ok(())
    } else {
        Err(format!("missing file: {}", path.display()).into())
    }
}

fn po_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = fs::read_dir(dir)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("po"))
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

fn workspace_source_root() -> Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| "could not determine workspace source root".into())
}

fn release_tag(args: Vec<String>) -> Result<()> {
    if matches!(args.as_slice(), [arg] if arg == "-h" || arg == "--help") {
        eprintln!("Usage: cargo run --locked -p xtask -- verify release-tag TAG");
        return Ok(());
    }
    if args.len() != 1 {
        return Err("verify release-tag requires TAG".into());
    }

    let tag = normalize_tag(args[0].trim_start_matches("refs/tags/"))?;
    let root = repo_root()?;
    env::set_current_dir(&root)?;

    let shallow = command_stdout("git", ["rev-parse", "--is-shallow-repository"])?;
    if shallow.trim() == "true" {
        run_command("git", ["fetch", "--force", "--unshallow", "origin"])?;
    }

    run_command(
        "git",
        [
            "fetch",
            "--force",
            "origin",
            "refs/heads/main:refs/remotes/origin/main",
            &format!("refs/tags/{tag}:refs/tags/{tag}"),
        ],
    )?;

    let tag_ref = format!("refs/tags/{tag}");
    let tag_type = command_stdout("git", ["cat-file", "-t", &tag_ref])?;
    if tag_type.trim() != "tag" {
        return Err("release ref must be an annotated signed tag".into());
    }

    let tag_contents = command_stdout("git", ["cat-file", "tag", &tag_ref])?;
    if !tag_contents
        .lines()
        .any(|line| line == "-----BEGIN PGP SIGNATURE-----")
    {
        return Err("release ref must contain a PGP signature".into());
    }

    let release_key = root.join(".github/release-gpg.pub");
    if release_key.is_file() {
        verify_tag_with_release_key(&tag_ref, &release_key)?;
    }

    let tag_target = command_stdout("git", ["rev-list", "-n1", &tag_ref])?;
    let head_commit = command_stdout("git", ["rev-parse", "HEAD"])?;
    if tag_target.trim() != head_commit.trim() {
        return Err("checked-out commit does not match release tag target".into());
    }

    run_command(
        "git",
        [
            "merge-base",
            "--is-ancestor",
            tag_target.trim(),
            "origin/main",
        ],
    )
}

fn verify_tag_with_release_key(tag_ref: &str, release_key: &Path) -> Result<()> {
    ensure_command("gpg")?;
    let gnupg_home = temp_path("gnupg");
    fs::create_dir(&gnupg_home)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&gnupg_home, fs::Permissions::from_mode(0o700))?;
    }

    let import_status = Command::new("gpg")
        .args(["--batch", "--import"])
        .arg(release_key)
        .env("GNUPGHOME", &gnupg_home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !import_status.success() {
        let _ = fs::remove_dir_all(&gnupg_home);
        return Err("failed to import release GPG key".into());
    }

    let verify_status = Command::new("git")
        .args(["verify-tag", tag_ref])
        .env("GNUPGHOME", &gnupg_home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    let _ = fs::remove_dir_all(&gnupg_home);
    if verify_status.success() {
        Ok(())
    } else {
        Err("release tag signature did not verify with .github/release-gpg.pub".into())
    }
}
