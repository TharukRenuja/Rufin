use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by Cargo"),
    );
    let icon_path = manifest_dir.join("../../packaging/windows/assets/rufin.ico");

    println!("cargo:rerun-if-changed={}", icon_path.display());

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon(icon_path.to_string_lossy().as_ref())
        .set("FileDescription", "Rufin")
        .set("ProductName", "Rufin")
        .set("OriginalFilename", "rufin.exe");
    resource.compile().expect("compile Windows resources");
}
