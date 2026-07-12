use library::{SourceId, Track};

use super::StoreHandle;

pub(in crate::controller) fn hydrate_source_tracks(
    store: &StoreHandle,
    source_id: &SourceId,
    tracks: &mut [Track],
) -> Result<(), String> {
    store.with_store_fast(|store| store.hydrate_tracks(source_id, tracks))
}
