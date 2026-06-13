use std::fs;
use std::path::Path;

#[test]
fn ui_uses_centralized_hidden_horizontal_scroll_policy() {
    let ui_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui");
    let mut offenders = Vec::new();
    collect_horizontal_external_policy_uses(&ui_dir, &mut offenders);

    assert!(
        offenders.is_empty(),
        "horizontal PolicyType::External must go through the centralized locked-width helper:\n{}",
        offenders.join("\n")
    );
}

fn collect_horizontal_external_policy_uses(dir: &Path, offenders: &mut Vec<String>) {
    let entries = fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", dir.display()));

    for entry in entries {
        let path = entry.expect("failed to read directory entry").path();
        if path.is_dir() {
            collect_horizontal_external_policy_uses(&path, offenders);
            continue;
        }
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        collect_file_offenders(&path, offenders);
    }
}

fn collect_file_offenders(path: &Path, offenders: &mut Vec<String>) {
    if path.ends_with("src/ui/layout.rs") {
        return;
    }

    let content = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));

    for (index, line) in content.lines().enumerate() {
        let horizontal_set_policy = line.contains("set_policy(gtk::PolicyType::External");
        let horizontal_spec = line.contains("horizontal_policy: gtk::PolicyType::External");
        if horizontal_set_policy || horizontal_spec {
            offenders.push(format!("{}:{}", path.display(), index + 1));
        }
    }
}
