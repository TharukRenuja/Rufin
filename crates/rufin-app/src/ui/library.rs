include!("library/paging.rs");
include!("library/routes_01.rs");
include!("library/routes_02.rs");
include!("library/collections.rs");
include!("library/album_detail.rs");
include!("library/cards.rs");

#[cfg(test)]
mod tests {
    include!("library/tests_01.rs");
}
