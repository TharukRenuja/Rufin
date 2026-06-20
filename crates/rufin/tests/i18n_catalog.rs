use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{self, Command},
};

#[test]
fn i18n_template_matches_generated_output() {
    let root = repo_root();
    let output = env::temp_dir().join(format!("rufin-template-{}.pot", process::id()));
    let output_run = Command::new(root.join(".github/scripts/update-i18n-template.sh"))
        .arg(&output)
        .current_dir(&root)
        .output()
        .expect("run i18n template update script");
    assert!(
        output_run.status.success(),
        "i18n template update script failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output_run.stdout),
        String::from_utf8_lossy(&output_run.stderr)
    );

    let generated = fs::read_to_string(&output)
        .unwrap_or_else(|error| panic!("read generated template {}: {error}", output.display()));
    let checked_in =
        fs::read_to_string(root.join("locales/rufin.pot")).expect("read checked-in i18n template");
    let _ = fs::remove_file(output);

    assert_eq!(
        checked_in, generated,
        "locales/rufin.pot is stale; run .github/scripts/update-i18n-template.sh"
    );
}

#[test]
fn i18n_files_omit_source_references() {
    let root = repo_root();
    let mut files = po_files(&root.join("locales"));
    files.push(root.join("locales/rufin.pot"));
    for file in files {
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|error| panic!("read {}: {error}", file.display()));
        assert!(
            !content.lines().any(|line| line.starts_with("#:")),
            "{} should omit source references",
            file.display()
        );
    }
}

#[test]
fn i18n_catalogs_pass_msgfmt_check() {
    let root = repo_root();
    let catalogs = po_files(&root.join("locales"));
    assert!(!catalogs.is_empty(), "expected at least one .po catalog");

    for catalog in catalogs {
        let stem = catalog
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("catalog");
        let output = env::temp_dir().join(format!("rufin-catalog-{}-{stem}.mo", process::id()));
        let status = Command::new("msgfmt")
            .arg("--check")
            .arg(&catalog)
            .arg("-o")
            .arg(&output)
            .status()
            .unwrap_or_else(|error| panic!("run msgfmt for {}: {error}", catalog.display()));
        let _ = fs::remove_file(output);
        assert!(
            status.success(),
            "msgfmt --check failed for {}",
            catalog.display()
        );
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("app crate lives under crates/rufin")
        .to_path_buf()
}

fn po_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("read {}: {error}", dir.display()))
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("po"))
        .collect::<Vec<_>>();
    files.sort();
    files
}
