include!("store/types.rs");
include!("store/store_lifecycle_schema.rs");
include!("store/library_cache_writes.rs");
include!("store/library_cache_reads.rs");
include!("store/library_auxiliary_cache.rs");
include!("store/library_search_helpers.rs");
include!("store/servers.rs");

#[cfg(test)]
mod tests {
    include!("store/schema_cache_tests.rs");
    include!("store/library_relationship_tests.rs");
    include!("store/sync_search_cover_tests.rs");
}
