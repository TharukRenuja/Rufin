use std::collections::HashSet;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use crate::process::{
    capture_command, capture_retry_without_ld_preload, collect_files_with_extension,
    ensure_command, find_on_path, find_on_path_os, path_to_slash, quoted_value, read_to_string,
    repo_root, temp_path, write_string,
};
use crate::release::normalize_plain_version;
use crate::{Result, parse_check_flag};

const CARGO_REGISTRY_SOURCE: &str = "registry+https://github.com/rust-lang/crates.io-index";
const FAKE_NIX_HASH: &str = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

pub(crate) fn run(mut args: Vec<String>) -> Result<()> {
    if args.is_empty() {
        return Err("missing generate command".into());
    }

    match args.remove(0).as_str() {
        "flatpak-sources" => flatpak_sources_command(args),
        "i18n-template" => i18n_template_command(args),
        "nix-cargo-hash" => nix_cargo_hash_command(args),
        "aur-stable" => aur_stable_command(args),
        command => Err(format!("unknown generate command: {command}").into()),
    }
}

fn flatpak_sources_command(args: Vec<String>) -> Result<()> {
    let Some(check) = parse_check_flag(
        args,
        "Usage: cargo xtask generate flatpak-sources [--check]",
    )?
    else {
        return Ok(());
    };
    flatpak_sources(check)
}

pub(crate) fn flatpak_sources(check: bool) -> Result<()> {
    let root = repo_root()?;
    let lock_file = root.join("Cargo.lock");
    let sources_file = root.join("packaging/flatpak/cargo-sources.json");
    let generated = generate_cargo_sources(&read_to_string(&lock_file)?)?;

    if check {
        let current = read_to_string(&sources_file)?;
        if current != generated {
            return Err(
                "packaging/flatpak/cargo-sources.json is stale; run cargo xtask generate flatpak-sources"
                    .into(),
            );
        }
        return Ok(());
    }

    write_string(&sources_file, &generated)?;
    Ok(())
}

#[derive(Default)]
struct CargoPackage {
    name: String,
    version: String,
    source: String,
    checksum: String,
}

fn generate_cargo_sources(lock: &str) -> Result<String> {
    let mut output = String::from("[\n");
    let mut current = CargoPackage::default();
    let mut in_package = false;
    let mut seen = HashSet::new();

    for line in lock.lines() {
        if line == "[[package]]" {
            if in_package {
                flush_cargo_package(&current, &mut seen, &mut output)?;
            }
            current = CargoPackage::default();
            in_package = true;
            continue;
        }

        if !in_package {
            continue;
        }

        if let Some(value) = quoted_value(line, "name") {
            current.name = value;
        } else if let Some(value) = quoted_value(line, "version") {
            current.version = value;
        } else if let Some(value) = quoted_value(line, "source") {
            current.source = value;
        } else if let Some(value) = quoted_value(line, "checksum") {
            current.checksum = value;
        }
    }

    if in_package {
        flush_cargo_package(&current, &mut seen, &mut output)?;
    }

    output.push_str("    {\n");
    output.push_str("        \"type\": \"inline\",\n");
    output.push_str(
        "        \"contents\": \"[source.vendored-sources]\\ndirectory = \\\"cargo/vendor\\\"\\n\\n[source.crates-io]\\nreplace-with = \\\"vendored-sources\\\"\\n\",\n",
    );
    output.push_str("        \"dest\": \"cargo\",\n");
    output.push_str("        \"dest-filename\": \"config\"\n");
    output.push_str("    }\n");
    output.push_str("]\n");
    Ok(output)
}

fn flush_cargo_package(
    package: &CargoPackage,
    seen: &mut HashSet<(String, String, String)>,
    output: &mut String,
) -> Result<()> {
    if package.source != CARGO_REGISTRY_SOURCE {
        return Ok(());
    }

    if package.checksum.is_empty() {
        return Err(format!("missing checksum for {} {}", package.name, package.version).into());
    }

    let key = (
        package.name.clone(),
        package.version.clone(),
        package.checksum.clone(),
    );
    if !seen.insert(key) {
        return Ok(());
    }

    let dest = format!("cargo/vendor/{}-{}", package.name, package.version);
    output.push_str("    {\n");
    output.push_str("        \"type\": \"archive\",\n");
    output.push_str("        \"archive-type\": \"tar-gzip\",\n");
    output.push_str(&format!(
        "        \"url\": \"https://static.crates.io/crates/{name}/{name}-{version}.crate\",\n",
        name = package.name,
        version = package.version
    ));
    output.push_str(&format!("        \"sha256\": \"{}\",\n", package.checksum));
    output.push_str(&format!("        \"dest\": \"{dest}\"\n"));
    output.push_str("    },\n");
    output.push_str("    {\n");
    output.push_str("        \"type\": \"inline\",\n");
    output.push_str(&format!(
        "        \"contents\": \"{{\\\"package\\\": \\\"{}\\\", \\\"files\\\": {{}}}}\",\n",
        package.checksum
    ));
    output.push_str(&format!("        \"dest\": \"{dest}\",\n"));
    output.push_str("        \"dest-filename\": \".cargo-checksum.json\"\n");
    output.push_str("    },\n");
    Ok(())
}

fn i18n_template_command(mut args: Vec<String>) -> Result<()> {
    let mut check = false;
    let mut output = PathBuf::from("locales/rufin.pot");

    while !args.is_empty() {
        match args.remove(0).as_str() {
            "--check" => check = true,
            "--output" => {
                if args.is_empty() {
                    return Err("--output requires a path".into());
                }
                output = PathBuf::from(args.remove(0));
            }
            "-h" | "--help" => {
                eprintln!("Usage: cargo xtask generate i18n-template [--check] [--output PATH]");
                return Ok(());
            }
            arg => return Err(format!("unexpected argument: {arg}").into()),
        }
    }

    if check {
        i18n_template_check()
    } else {
        i18n_template_to(&output)
    }
}

pub(crate) fn i18n_template_check() -> Result<()> {
    let root = repo_root()?;
    let output = temp_path("i18n-template.pot");
    i18n_template_to(&output)?;
    let generated = read_to_string(&output)?;
    let checked_in = read_to_string(&root.join("locales/rufin.pot"))?;
    let _ = fs::remove_file(&output);
    if checked_in == generated {
        Ok(())
    } else {
        Err("locales/rufin.pot is stale; run cargo xtask generate i18n-template".into())
    }
}

pub(crate) fn i18n_template_to(output: &Path) -> Result<()> {
    ensure_command("xgettext")?;
    let root = repo_root()?;
    let tmp_dir = root.join("target/tmp");
    fs::create_dir_all(&tmp_dir)?;
    let sources = tmp_dir.join(format!("i18n-sources-{}.txt", std::process::id()));
    let entries = tmp_dir.join(format!("i18n-entries-{}.pot", std::process::id()));

    let result = write_i18n_template(&root, &sources, &entries, output);
    let _ = fs::remove_file(&sources);
    let _ = fs::remove_file(&entries);
    result
}

fn write_i18n_template(root: &Path, sources: &Path, entries: &Path, output: &Path) -> Result<()> {
    let mut rust_files = Vec::new();
    collect_files_with_extension(root, &root.join("crates/rufin/src"), "rs", &mut rust_files)?;
    collect_files_with_extension(root, &root.join("crates/domain/src"), "rs", &mut rust_files)?;
    rust_files.sort();

    let mut source_list = fs::File::create(sources)?;
    for file in rust_files {
        writeln!(source_list, "{}", path_to_slash(&file))?;
    }

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }

    let status = Command::new("xgettext")
        .current_dir(root)
        .args([
            "--from-code=UTF-8",
            "--language=Rust",
            "--escape",
            "--no-location",
            "--sort-by-file",
            "--package-name=Rufin",
            "--msgid-bugs-address=https://github.com/screwys/Rufin/issues",
            "--keyword=tr:1",
            "--keyword=tr_with:1",
            "--keyword=trn:1,2",
            "--keyword=trn_with:1,2",
            "--keyword=msgid:1",
            "--keyword=text_button:2",
            "--keyword=icon_button:2",
            "--keyword=detail_action_button:2",
            "--keyword=detail_link_button:2",
            "--keyword=toggle_button:2",
            "--keyword=row_button:2",
            "--keyword=cover_hover_controls:2",
            "--keyword=relocalize_icon_button:2",
            "--keyword=context_menu_action:1",
            "--keyword=context_menu_picker_button:1",
            "--keyword=table_header_label:1",
            "--keyword=button_row:1",
            "--keyword=dialog_button:1",
            "--keyword=labeled_control:1",
            "--keyword=labeled_row:1",
            "--keyword=smart_playlist_dialog:1",
        ])
        .arg(format!("--files-from={}", sources.display()))
        .arg(format!("--output={}", entries.display()))
        .stdin(Stdio::inherit())
        .output()?;
    let stdout = String::from_utf8_lossy(&status.stdout);
    let stderr = String::from_utf8_lossy(&status.stderr);
    if !status.status.success() {
        return Err(format!(
            "xgettext failed with status {}\nstdout:\n{stdout}\nstderr:\n{stderr}",
            status.status
        )
        .into());
    }
    if !stderr.trim().is_empty() {
        return Err(format!("xgettext emitted warnings:\n{stderr}").into());
    }

    let mut template = String::from(
        "# Rufin translation template.\n# Copyright (C) 2026 Rufin contributors\n# This file is distributed under the same license as the Rufin package.\n#\n#, fuzzy\nmsgid \"\"\nmsgstr \"\"\n\"Project-Id-Version: Rufin\\n\"\n\"Report-Msgid-Bugs-To: https://github.com/screwys/Rufin/issues\\n\"\n\"POT-Creation-Date: YEAR-MO-DA HO:MI+ZONE\\n\"\n\"PO-Revision-Date: YEAR-MO-DA HO:MI+ZONE\\n\"\n\"Last-Translator: Rufin translators\\n\"\n\"Language-Team: Rufin translators\\n\"\n\"Language: \\n\"\n\"MIME-Version: 1.0\\n\"\n\"Content-Type: text/plain; charset=UTF-8\\n\"\n\"Content-Transfer-Encoding: 8bit\\n\"\n",
    );
    if entries.metadata()?.len() > 0 {
        template.push('\n');
        template.push_str(strip_xgettext_header(&read_to_string(entries)?));
    }
    write_string(output, &template)
}

fn strip_xgettext_header(input: &str) -> &str {
    input
        .split_once("\n\n")
        .map_or(input, |(_, entries)| entries)
}

fn nix_cargo_hash_command(args: Vec<String>) -> Result<()> {
    let Some(check) =
        parse_check_flag(args, "Usage: cargo xtask generate nix-cargo-hash [--check]")?
    else {
        return Ok(());
    };
    nix_cargo_hash(check)
}

pub(crate) fn nix_cargo_hash(check: bool) -> Result<()> {
    let root = repo_root()?;
    let flake_file = root.join("flake.nix");
    let original = read_to_string(&flake_file)?;
    let current_hash =
        cargo_hash_from_flake(&original).ok_or("could not find cargoHash in flake.nix")?;
    let fake_flake = replace_cargo_hash(&original, FAKE_NIX_HASH)?;
    write_string(&flake_file, &fake_flake)?;

    let output = capture_retry_without_ld_preload([
        "env",
        "-u",
        "LD_PRELOAD",
        "nix",
        "--accept-flake-config",
        "--extra-experimental-features",
        "nix-command flakes",
        "build",
        ".#rufin",
        "--no-link",
        "--print-build-logs",
    ]);

    let output = match output {
        Ok(output) => output,
        Err(error) => {
            write_string(&flake_file, &original)?;
            return Err(error);
        }
    };
    let combined = format!("{}{}", output.stdout, output.stderr);
    let new_hash = cargo_hash_from_nix_output(&combined);

    let Some(new_hash) = new_hash else {
        write_string(&flake_file, &original)?;
        eprint!("{combined}");
        return Err("could not determine cargoHash".into());
    };

    if new_hash == current_hash {
        write_string(&flake_file, &original)?;
        println!("cargoHash is already up to date: {new_hash}");
    } else if check {
        write_string(&flake_file, &original)?;
        return Err(format!("cargoHash is stale: {current_hash} -> {new_hash}").into());
    } else {
        let updated = replace_cargo_hash(&original, &new_hash)?;
        write_string(&flake_file, &updated)?;
        println!("updated cargoHash: {current_hash} -> {new_hash}");
    }

    Ok(())
}

fn cargo_hash_from_flake(input: &str) -> Option<String> {
    input.lines().find_map(|line| {
        line.trim()
            .strip_prefix("cargoHash = \"")
            .and_then(|value| value.strip_suffix("\";"))
            .map(ToOwned::to_owned)
    })
}

fn cargo_hash_from_nix_output(input: &str) -> Option<String> {
    input.lines().rev().find_map(|line| {
        line.trim()
            .strip_prefix("got:")
            .map(str::trim)
            .filter(|value| value.starts_with("sha256-"))
            .map(ToOwned::to_owned)
    })
}

fn replace_cargo_hash(input: &str, hash: &str) -> Result<String> {
    let mut output = String::new();
    let mut replaced = false;
    for line in input.lines() {
        let trimmed = line.trim_start();
        if !replaced && trimmed.starts_with("cargoHash = \"") && trimmed.ends_with("\";") {
            let indent_len = line.len() - trimmed.len();
            output.push_str(&line[..indent_len]);
            output.push_str(&format!("cargoHash = \"{hash}\";\n"));
            replaced = true;
        } else {
            output.push_str(line);
            output.push('\n');
        }
    }
    if replaced {
        Ok(output)
    } else {
        Err("could not find cargoHash in flake.nix".into())
    }
}

fn aur_stable_command(mut args: Vec<String>) -> Result<()> {
    let mut check = false;
    let mut skip_srcinfo = false;

    while !args.is_empty() {
        match args.remove(0).as_str() {
            "--check" => check = true,
            "--skip-srcinfo" => skip_srcinfo = true,
            "-h" | "--help" => {
                eprintln!(
                    "Usage: cargo xtask generate aur-stable [--check] [--skip-srcinfo] VERSION"
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
        return Err("generate aur-stable requires VERSION".into());
    }
    aur_stable(check, skip_srcinfo, &args[0])
}

pub(crate) fn aur_stable(check: bool, skip_srcinfo: bool, version: &str) -> Result<()> {
    let root = repo_root()?;
    let pkgbuild = root.join("packaging/aur/rufin/PKGBUILD");
    let srcinfo = root.join("packaging/aur/rufin/.SRCINFO");
    let original_pkgbuild = if check {
        Some(read_to_string(&pkgbuild)?)
    } else {
        None
    };
    let original_srcinfo = if check {
        Some(read_to_string(&srcinfo)?)
    } else {
        None
    };

    let result = aur_stable_inner(skip_srcinfo, version);
    if !check {
        return result;
    }

    let expected_pkgbuild = read_to_string(&pkgbuild).ok();
    let expected_srcinfo = read_to_string(&srcinfo).ok();
    if let Some(original) = &original_pkgbuild {
        write_string(&pkgbuild, original)?;
    }
    if let Some(original) = &original_srcinfo {
        write_string(&srcinfo, original)?;
    }
    result?;

    let original_pkgbuild =
        original_pkgbuild.ok_or("failed to keep original PKGBUILD for check mode")?;
    let original_srcinfo =
        original_srcinfo.ok_or("failed to keep original .SRCINFO for check mode")?;
    let expected_pkgbuild = expected_pkgbuild.ok_or("failed to read expected PKGBUILD")?;
    let expected_srcinfo = expected_srcinfo.ok_or("failed to read expected .SRCINFO")?;

    if original_pkgbuild == expected_pkgbuild && original_srcinfo == expected_srcinfo {
        return Ok(());
    }

    eprintln!(
        "Checked-in stable AUR metadata is not in sync with v{}.",
        normalize_plain_version(version)?
    );
    eprintln!("Run:");
    eprintln!(
        "  cargo xtask generate aur-stable v{}",
        normalize_plain_version(version)?
    );
    print_diff(
        "packaging/aur/rufin/PKGBUILD (current)",
        &original_pkgbuild,
        "packaging/aur/rufin/PKGBUILD (expected)",
        &expected_pkgbuild,
    );
    print_diff(
        "packaging/aur/rufin/.SRCINFO (current)",
        &original_srcinfo,
        "packaging/aur/rufin/.SRCINFO (expected)",
        &expected_srcinfo,
    );
    Err("stable AUR metadata is stale".into())
}

fn aur_stable_inner(skip_srcinfo: bool, version: &str) -> Result<()> {
    let version = normalize_plain_version(version)?;
    let root = repo_root()?;
    let pkgdir = root.join("packaging/aur/rufin");
    let pkgbuild = pkgdir.join("PKGBUILD");
    let srcinfo = pkgdir.join(".SRCINFO");
    if !pkgbuild.is_file() {
        return Err(format!("missing stable AUR PKGBUILD: {}", pkgbuild.display()).into());
    }

    let original_pkgbuild = read_to_string(&pkgbuild)?;
    let original_srcinfo = read_to_string(&srcinfo).ok();
    let result = (|| {
        let repo = env::var("GITHUB_REPOSITORY").unwrap_or_else(|_| "screwys/Rufin".to_owned());
        let archive_url = format!("https://github.com/{repo}/archive/refs/tags/v{version}.tar.gz");
        let checksum = archive_checksum(&archive_url)?;

        update_pkgbuild(&pkgbuild, &version, &checksum)?;
        if !skip_srcinfo && !refresh_srcinfo(&pkgdir, &srcinfo)? {
            if env::var("RUFIN_AUR_REQUIRE_MAKEPKG").unwrap_or_default() == "1" {
                return Err("makepkg unavailable; refusing field-only .SRCINFO update".into());
            }
            update_srcinfo_fields(&srcinfo, &version, &checksum)?;
            eprintln!(
                "makepkg unavailable; updated .SRCINFO release fields without regenerating dependencies"
            );
        }
        Ok(())
    })();

    if result.is_err() {
        let _ = write_string(&pkgbuild, &original_pkgbuild);
        if let Some(original_srcinfo) = original_srcinfo {
            let _ = write_string(&srcinfo, &original_srcinfo);
        }
    }
    result
}

fn archive_checksum(url: &str) -> Result<String> {
    ensure_command("curl")?;
    ensure_command("sha256sum")?;
    let archive = temp_path("aur-archive.tar.gz");
    for attempt in 1..=5 {
        let curl = Command::new("curl")
            .args(["-LfsS", "-o"])
            .arg(&archive)
            .arg(url)
            .status();
        if matches!(curl, Ok(status) if status.success()) {
            let output = Command::new("sha256sum").arg(&archive).output()?;
            let _ = fs::remove_file(&archive);
            if output.status.success() {
                let stdout = String::from_utf8(output.stdout)?;
                let checksum = stdout.split_whitespace().next().unwrap_or_default();
                if !checksum.is_empty() {
                    return Ok(checksum.to_owned());
                }
            }
        }

        if attempt == 5 {
            let _ = fs::remove_file(&archive);
            return Err(format!("failed to fetch release archive: {url}").into());
        }
        thread::sleep(Duration::from_secs(2));
    }
    unreachable!("attempt loop returns on success or final failure")
}

fn update_pkgbuild(path: &Path, version: &str, checksum: &str) -> Result<()> {
    let input = read_to_string(path)?;
    let mut output = String::new();
    let mut version_replaced = false;
    let mut checksum_replaced = false;
    for line in input.lines() {
        if line.starts_with("pkgver=") {
            output.push_str(&format!("pkgver={version}\n"));
            version_replaced = true;
        } else if line.starts_with("sha256sums=") {
            output.push_str(&format!("sha256sums=('{checksum}')\n"));
            checksum_replaced = true;
        } else {
            output.push_str(line);
            output.push('\n');
        }
    }
    if !version_replaced {
        return Err("missing pkgver in PKGBUILD".into());
    }
    if !checksum_replaced {
        return Err("missing sha256sums in PKGBUILD".into());
    }
    write_string(path, &output)
}

fn refresh_srcinfo(pkgdir: &Path, srcinfo: &Path) -> Result<bool> {
    if refresh_srcinfo_with_native_makepkg(pkgdir, srcinfo)? {
        return Ok(true);
    }
    if refresh_srcinfo_with_profile_makepkg(pkgdir, srcinfo)? {
        return Ok(true);
    }
    refresh_srcinfo_with_nix_makepkg(pkgdir, srcinfo)
}

fn refresh_srcinfo_with_native_makepkg(pkgdir: &Path, srcinfo: &Path) -> Result<bool> {
    let Some(makepkg) = find_on_path_os("makepkg") else {
        return Ok(false);
    };
    let makepkg_prefix = makepkg
        .parent()
        .and_then(Path::parent)
        .ok_or("could not determine makepkg prefix")?;
    let config = makepkg_prefix.join("etc/makepkg.conf");

    let output = if config.is_file() {
        Command::new(&makepkg)
            .args(["--config"])
            .arg(&config)
            .arg("--printsrcinfo")
            .current_dir(pkgdir)
            .output()?
    } else {
        Command::new(&makepkg)
            .arg("--printsrcinfo")
            .current_dir(pkgdir)
            .output()?
    };
    if !output.status.success() {
        return Ok(false);
    }
    write_srcinfo_from_stdout(srcinfo, &String::from_utf8_lossy(&output.stdout))
}

fn refresh_srcinfo_with_profile_makepkg(pkgdir: &Path, srcinfo: &Path) -> Result<bool> {
    if !find_on_path("nix-profile-exec") || !find_on_path("nix") {
        return Ok(false);
    }
    let Some(config) = profile_makepkg_config()? else {
        return Ok(false);
    };

    let output = Command::new("nix-profile-exec")
        .arg("makepkg")
        .arg("--config")
        .arg(&config)
        .arg("--printsrcinfo")
        .current_dir(pkgdir)
        .env_remove("LD_PRELOAD")
        .output()?;

    if output.status.success()
        && write_srcinfo_from_stdout(srcinfo, &String::from_utf8_lossy(&output.stdout))?
    {
        return Ok(true);
    }

    Ok(false)
}

fn profile_makepkg_config() -> Result<Option<PathBuf>> {
    let output = Command::new("env")
        .args(["-u", "LD_PRELOAD", "nix", "profile", "list", "--json"])
        .output()?;
    if !output.status.success() {
        return Ok(None);
    }

    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let Some(store_path) = value
        .get("elements")
        .and_then(|elements| elements.get("pacman"))
        .and_then(|pacman| pacman.get("storePaths"))
        .and_then(serde_json::Value::as_array)
        .and_then(|paths| paths.first())
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(None);
    };

    Ok(Some(PathBuf::from(store_path).join("etc/makepkg.conf")))
}

fn refresh_srcinfo_with_nix_makepkg(pkgdir: &Path, srcinfo: &Path) -> Result<bool> {
    if !find_on_path("nix") {
        return Ok(false);
    }
    let timeout_seconds =
        env::var("RUFIN_AUR_NIX_MAKEPKG_TIMEOUT_SECONDS").unwrap_or_else(|_| "30".to_owned());
    if timeout_seconds
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .is_none()
    {
        eprintln!("RUFIN_AUR_NIX_MAKEPKG_TIMEOUT_SECONDS must be a positive integer");
        return Ok(false);
    }

    let script = r#"
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
timeout "$2" makepkg --config "$makepkg_config" --printsrcinfo
"#;
    let mut args = vec![
        OsString::from("env"),
        OsString::from("-u"),
        OsString::from("LD_PRELOAD"),
        OsString::from("nix"),
        OsString::from("--accept-flake-config"),
        OsString::from("--extra-experimental-features"),
        OsString::from("nix-command flakes"),
        OsString::from("shell"),
        OsString::from("nixpkgs#pacman"),
        OsString::from("--command"),
        OsString::from("env"),
        OsString::from("-u"),
        OsString::from("LD_PRELOAD"),
        OsString::from("bash"),
        OsString::from("-lc"),
        OsString::from(script),
        OsString::from("bash"),
    ];
    args.push(pkgdir.as_os_str().to_owned());
    args.push(OsString::from(format!("{timeout_seconds}s")));

    let output = capture_retry_without_ld_preload(args)?;
    if !output.status.success() {
        if env::var("RUFIN_AUR_REQUIRE_MAKEPKG").unwrap_or_default() == "1" {
            eprint!("{}", output.stdout);
            eprint!("{}", output.stderr);
        }
        return Ok(false);
    }
    if write_srcinfo_from_stdout(srcinfo, &output.stdout)? {
        Ok(true)
    } else {
        if env::var("RUFIN_AUR_REQUIRE_MAKEPKG").unwrap_or_default() == "1" {
            eprint!("{}", output.stdout);
            eprint!("{}", output.stderr);
        }
        Ok(false)
    }
}

fn write_srcinfo_from_stdout(srcinfo: &Path, stdout: &str) -> Result<bool> {
    let Some(srcinfo_output) = extract_srcinfo_output(stdout) else {
        return Ok(false);
    };
    write_string(srcinfo, &srcinfo_output)?;
    Ok(true)
}

fn extract_srcinfo_output(stdout: &str) -> Option<String> {
    let mut lines = stdout
        .lines()
        .skip_while(|line| line.trim() != "pkgbase = rufin");
    let first = lines.next()?;
    let mut output = String::new();
    output.push_str(first);
    output.push('\n');
    for line in lines {
        output.push_str(line);
        output.push('\n');
    }
    Some(output)
}

fn update_srcinfo_fields(path: &Path, version: &str, checksum: &str) -> Result<()> {
    let input = read_to_string(path)?;
    let mut output = String::new();
    let mut version_replaced = false;
    let mut source_replaced = false;
    let mut checksum_replaced = false;

    for line in input.lines() {
        let trimmed = line.trim_start();
        let indent = &line[..line.len() - trimmed.len()];
        if trimmed.starts_with("pkgver = ") {
            output.push_str(&format!("{indent}pkgver = {version}\n"));
            version_replaced = true;
        } else if trimmed.starts_with("source = rufin-")
            && trimmed.contains("::https://github.com/screwys/Rufin/archive/refs/tags/v")
            && trimmed.ends_with(".tar.gz")
        {
            output.push_str(&format!(
                "{indent}source = rufin-{version}.tar.gz::https://github.com/screwys/Rufin/archive/refs/tags/v{version}.tar.gz\n"
            ));
            source_replaced = true;
        } else if trimmed.starts_with("sha256sums = ") {
            output.push_str(&format!("{indent}sha256sums = {checksum}\n"));
            checksum_replaced = true;
        } else {
            output.push_str(line);
            output.push('\n');
        }
    }

    if !version_replaced {
        return Err("missing pkgver in .SRCINFO".into());
    }
    if !source_replaced {
        return Err("missing source in .SRCINFO".into());
    }
    if !checksum_replaced {
        return Err("missing sha256sums in .SRCINFO".into());
    }
    write_string(path, &output)
}

fn print_diff(current_label: &str, current: &str, expected_label: &str, expected: &str) {
    let current_file = temp_path("current.diff");
    let expected_file = temp_path("expected.diff");
    if write_string(&current_file, current).is_ok()
        && write_string(&expected_file, expected).is_ok()
        && let Ok(output) = capture_command(
            "diff",
            [
                "-u",
                "--label",
                current_label,
                "--label",
                expected_label,
                current_file.to_str().unwrap_or_default(),
                expected_file.to_str().unwrap_or_default(),
            ],
        )
    {
        eprint!("{}", output.stdout);
        eprint!("{}", output.stderr);
    }
    let _ = fs::remove_file(current_file);
    let _ = fs::remove_file(expected_file);
}

#[cfg(test)]
mod tests {
    use super::extract_srcinfo_output;

    #[test]
    fn srcinfo_output_ignores_leading_nix_noise() {
        let output = extract_srcinfo_output(
            "copying path '/nix/store/example-pacman'\n\
             pkgbase = rufin\n\
             \tpkgver = 0.7.12\n\
             pkgname = rufin\n",
        )
        .expect("expected .SRCINFO output");

        assert_eq!(
            output,
            "pkgbase = rufin\n\tpkgver = 0.7.12\npkgname = rufin\n"
        );
    }
}
