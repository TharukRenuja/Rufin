include!("library/paging.rs");
include!("library/routes.rs");
include!("library/route_shell.rs");
include!("library/collections.rs");
include!("library/album_detail.rs");
include!("library/cards.rs");

#[cfg(test)]
mod tests {
    include!("library/route_tests.rs");
}
