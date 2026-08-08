fn main() {
    glib_build_tools::compile_resources(
        &["../../data/icons"],
        "../../data/icons/rufin-icons.gresource.xml",
        "rufin.gresource",
    );
}
