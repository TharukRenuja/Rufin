#[path = "preferences/library.rs"]
mod library;

include!("preferences/root/dialog.rs");
include!("preferences/root/general.rs");
include!("preferences/root/layout.rs");

#[cfg(test)]
mod tests {
    include!("preferences/root/tests_01.rs");
}
