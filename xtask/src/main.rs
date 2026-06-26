use std::collections::HashSet;
use std::env;
use std::error::Error;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

mod release;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const CARGO_REGISTRY_SOURCE: &str = "registry+https://github.com/rust-lang/crates.io-index";

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        print_usage();
        return Err("missing command".into());
    }

    match args.remove(0).as_str() {
        "check" => run_check(args),
        "flatpak" => run_flatpak(args),
        "release" => release::run(args),
        "-h" | "--help" => {
            print_usage();
            Ok(())
        }
        command => Err(format!("unknown command: {command}").into()),
    }
}

fn print_usage() {
    eprintln!(
        "Usage:
  cargo xtask check release-local [--base-ref REF]
  cargo xtask flatpak update-cargo-sources [--check]
  cargo xtask flatpak check-icon-assertions
  cargo xtask release prepare VERSION SUMMARY
  cargo xtask release update-flathub-manifest [--manifest PATH] TAG"
    );
}

fn run_check(mut args: Vec<String>) -> Result<()> {
    if args.is_empty() {
        return Err("missing check command".into());
    }

    match args.remove(0).as_str() {
        "release-local" => check_release_local(args),
        command => Err(format!("unknown check command: {command}").into()),
    }
}

fn run_flatpak(mut args: Vec<String>) -> Result<()> {
    if args.is_empty() {
        return Err("missing flatpak command".into());
    }

    match args.remove(0).as_str() {
        "update-cargo-sources" => {
            let Some(check) = parse_check_flag(args)? else {
                return Ok(());
            };
            flatpak_update_cargo_sources(check)
        }
        "check-icon-assertions" => {
            if print_help_if_requested(&args, "Usage: cargo xtask flatpak check-icon-assertions")? {
                return Ok(());
            }
            ensure_no_args(&args)?;
            flatpak_check_icon_assertions()
        }
        command => Err(format!("unknown flatpak command: {command}").into()),
    }
}

fn parse_check_flag(args: Vec<String>) -> Result<Option<bool>> {
    let mut check = false;
    for arg in args {
        match arg.as_str() {
            "--check" => check = true,
            "-h" | "--help" => {
                eprintln!("Usage: cargo xtask flatpak update-cargo-sources [--check]");
                return Ok(None);
            }
            _ => return Err(format!("unexpected argument: {arg}").into()),
        }
    }
    Ok(Some(check))
}

fn ensure_no_args(args: &[String]) -> Result<()> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(format!("unexpected argument: {}", args[0]).into())
    }
}

fn print_help_if_requested(args: &[String], usage: &str) -> Result<bool> {
    match args {
        [arg] if arg == "-h" || arg == "--help" => {
            eprintln!("{usage}");
            Ok(true)
        }
        [arg, ..] if arg == "-h" || arg == "--help" => {
            Err(format!("unexpected argument after {arg}: {}", args[1]).into())
        }
        _ => Ok(false),
    }
}

pub(crate) fn repo_root() -> Result<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()?;
    if !output.status.success() {
        return Err("could not determine repository root".into());
    }
    let root = String::from_utf8(output.stdout)?;
    Ok(PathBuf::from(root.trim()))
}

pub(crate) fn command_stdout<I, S>(program: &str, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new(program).args(args).output()?;
    if !output.status.success() {
        return Err(format!("{program} failed with status {}", output.status).into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

pub(crate) fn run_command<I, S>(program: &str, args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let status = Command::new(program)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} failed with status {status}").into())
    }
}

fn run_bash_without_ld_preload(args: &[&str]) -> Result<()> {
    let status = Command::new("bash")
        .args(args)
        .env_remove("LD_PRELOAD")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("bash failed with status {status}").into())
    }
}

fn flatpak_update_cargo_sources(check: bool) -> Result<()> {
    let root = repo_root()?;
    let lock_file = root.join("Cargo.lock");
    let sources_file = root.join("packaging/flatpak/cargo-sources.json");
    let generated = generate_cargo_sources(&read_to_string(&lock_file)?)?;

    if check {
        let current = read_to_string(&sources_file)?;
        if current != generated {
            return Err(
                "packaging/flatpak/cargo-sources.json is stale; run cargo xtask flatpak update-cargo-sources"
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

fn quoted_value(line: &str, key: &str) -> Option<String> {
    let prefix = format!("{key} = \"");
    line.strip_prefix(&prefix)
        .and_then(|value| value.strip_suffix('"'))
        .map(ToOwned::to_owned)
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

fn flatpak_check_icon_assertions() -> Result<()> {
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

fn check_release_local(mut args: Vec<String>) -> Result<()> {
    let mut base_ref =
        env::var("RUFIN_RELEASE_CHECK_BASE_REF").unwrap_or_else(|_| "origin/main".to_owned());

    while !args.is_empty() {
        match args.remove(0).as_str() {
            "--base-ref" => {
                if args.is_empty() {
                    return Err("--base-ref requires a ref".into());
                }
                base_ref = args.remove(0);
            }
            "-h" | "--help" => {
                eprintln!("Usage: cargo xtask check release-local [--base-ref REF]");
                return Ok(());
            }
            arg => return Err(format!("unexpected argument: {arg}").into()),
        }
    }

    ensure_command("cargo")?;
    ensure_command("cargo-deny")?;

    let root = repo_root()?;
    env::set_current_dir(&root)?;
    let base_commit = command_stdout("git", ["merge-base", "HEAD", &base_ref])?;
    let changed_paths = changed_paths_since(base_commit.trim())?;
    let needs_nix_hash_check = changed_paths.iter().any(|path| {
        path == "Cargo.lock"
            || path == "Cargo.toml"
            || path.ends_with("/Cargo.toml")
            || path == "flake.nix"
            || path == ".github/scripts/update-nix-cargo-hash.sh"
    });

    flatpak_update_cargo_sources(true)?;
    if needs_nix_hash_check {
        run_bash_without_ld_preload(&[".github/scripts/update-nix-cargo-hash.sh", "--check"])?;
    }
    run_command("cargo", ["fmt", "--all", "--", "--check"])?;
    run_command("bash", ["scripts/lint-rust.sh"])?;
    run_command("bash", ["scripts/test-rust.sh"])?;
    run_command("bash", ["scripts/check-deps.sh"])?;

    Ok(())
}

fn changed_paths_since(base_commit: &str) -> Result<HashSet<String>> {
    let mut paths = HashSet::new();
    let head_diff = format!("{base_commit}...HEAD");
    for args in [
        vec!["diff".to_owned(), "--name-only".to_owned(), head_diff],
        vec!["diff".to_owned(), "--name-only".to_owned()],
        vec![
            "diff".to_owned(),
            "--cached".to_owned(),
            "--name-only".to_owned(),
        ],
        vec![
            "ls-files".to_owned(),
            "--others".to_owned(),
            "--exclude-standard".to_owned(),
        ],
    ] {
        let output = command_stdout("git", args)?;
        paths.extend(
            output
                .lines()
                .filter(|path| !path.is_empty())
                .map(ToOwned::to_owned),
        );
    }
    Ok(paths)
}

fn ensure_command(command: &str) -> Result<()> {
    if find_on_path(command) {
        Ok(())
    } else {
        Err(format!("{command} is required").into())
    }
}

fn find_on_path(command: &str) -> bool {
    let Some(paths) = env::var_os("PATH") else {
        return false;
    };

    env::split_paths(&paths).any(|path| path.join(command).is_file())
}

fn collect_files_relative(
    root: &Path,
    current: &Path,
    output: &mut Vec<PathBuf>,
) -> io::Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files_relative(root, &path, output)?;
        } else if path.is_file() {
            let relative = path.strip_prefix(root).map_err(io::Error::other)?;
            output.push(relative.to_path_buf());
        }
    }
    Ok(())
}

fn path_to_slash(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

pub(crate) fn read_to_string(path: &Path) -> Result<String> {
    fs::read_to_string(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()).into())
}

pub(crate) fn write_string(path: &Path, contents: &str) -> Result<()> {
    fs::write(path, contents)
        .map_err(|err| format!("failed to write {}: {err}", path.display()).into())
}
