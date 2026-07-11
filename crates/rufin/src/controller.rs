mod covers;

mod discovery;

mod generated_radio;
pub(crate) use generated_radio::{
    cached_generated_track_executor, native_generated_track_executor,
};

mod source_tracks;

mod random;

mod root;

pub use root::*;
pub(in crate::controller) use root::{
    IMAGE_TAG_UNTAGGED, acquire_cover_slot, cover_cache_path_for_key, is_local_source_image_ref,
    release_cover_slot,
};
pub(crate) use root::{StoreHandle, load_settings_from_store};
pub(crate) use root::{cached_auto_dj_operation, native_auto_dj_operation};
pub(crate) use root::{grouped_cover_refs_for_items, track_cover_refs_for_items};
pub(crate) use root::{local_sync_operation, remote_sync_operation};
