mod covers;

mod discovery;

mod random;

mod root;

pub use root::*;
pub(in crate::controller) use root::{
    IMAGE_TAG_UNTAGGED, SNAPSHOT_TRACK_LIMIT, StoreHandle, acquire_cover_slot,
    cover_cache_path_for_key, load_settings_for_saved, load_settings_from_store,
    local_folder_paths, provider_for_saved, release_cover_slot,
};
pub(crate) use root::{grouped_cover_refs_for_items, track_cover_refs_for_items};
